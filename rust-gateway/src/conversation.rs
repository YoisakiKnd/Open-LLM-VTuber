//! Rust-native conversation orchestration (M1.5 / M2 wiring).
//!
//! In `--chat-mode native` the gateway intercepts conversation messages
//! (`text-input`, `ai-speak-signal`, `interrupt-signal`) and drives a
//! [`ChatSession`] backed by a native [`ChatProvider`] (OpenAI / Anthropic /
//! Ollama) instead of forwarding them to the Python runtime. Audio input
//! transcription and TTS remain on the Python sidecar (see the M2 decision
//! matrix): `mic-audio-end` is only routed to the orchestrator once a
//! transcription is available, and assistant replies are delivered as
//! `full-text` events with the Python TTS path following in a later step.
//!
//! Interrupt semantics: every new turn cancels the in-flight turn via
//! [`CancellationToken`]; the provider call observes cancellation and
//! unwinds promptly.
//!
//! A few convenience APIs (`run_turn`, `is_turn_active`, `history`) are used
//! by the unit tests to drive the state machine directly; production code
//! composes `take_active_turn` + [`run_active_turn`] instead.
#![cfg_attr(not(test), allow(dead_code))]

use crate::cancellation::{CancellationGuard, CancellationToken};
use crate::legacy_settings::LegacySettingsAdapter;
use crate::provider::{
    ChatMessage, ChatProvider, ProviderError, ProviderRequest, ProviderResponse, ProviderRole,
};

/// Default cap on conversation history messages retained per session.
pub const DEFAULT_HISTORY_LIMIT: usize = 20;

/// Outcome of one orchestrated turn.
#[derive(Debug)]
pub enum TurnOutcome {
    /// The provider produced a reply (possibly empty).
    Completed(ProviderResponse),
    /// The turn was cancelled (interrupt) before completion.
    Cancelled,
    /// The provider failed with a classified error.
    Failed(ProviderError),
}

/// An in-flight turn: the request snapshot plus its cancellation token.
/// Owned by the orchestrator task so that new inputs can interrupt a running
/// provider call without holding the [`ChatSession`] borrow.
#[derive(Debug)]
pub struct ActiveTurn {
    pub messages: Vec<ChatMessage>,
    pub token: CancellationToken,
}

/// Runs one turn against the provider outside the session borrow.
/// The reply is *not* recorded here; the caller re-inserts it via
/// [`ChatSession::record_reply`] once the outcome is known.
pub async fn run_active_turn(
    provider: std::sync::Arc<dyn ChatProvider>,
    turn: ActiveTurn,
) -> TurnOutcome {
    let request = ProviderRequest {
        messages: turn.messages,
        ..Default::default()
    };
    let _guard = CancellationGuard::new(turn.token.clone());
    match provider.complete(&request, &turn.token).await {
        Ok(response) => TurnOutcome::Completed(response),
        Err(ProviderError::Cancelled) => TurnOutcome::Cancelled,
        Err(error) => TurnOutcome::Failed(error),
    }
}

/// Rust-side conversational state for one session.
pub struct ChatSession {
    provider: std::sync::Arc<dyn ChatProvider>,
    messages: Vec<ChatMessage>,
    history_limit: usize,
    system_prompt: Option<String>,
    token: CancellationToken,
    active_turn: bool,
}

impl ChatSession {
    pub fn new(
        provider: std::sync::Arc<dyn ChatProvider>,
        history_limit: usize,
        system_prompt: Option<String>,
    ) -> Self {
        let mut session = Self {
            provider,
            messages: Vec::new(),
            history_limit: history_limit.max(1),
            system_prompt: None,
            token: CancellationToken::new(),
            active_turn: false,
        };
        session.set_character_prompt(system_prompt);
        session
    }

    /// Updates the system prompt (e.g. on character switch) and rebuilds the
    /// leading system message in the history.
    pub fn set_character_prompt(&mut self, prompt: Option<String>) {
        if self.system_prompt == prompt {
            return;
        }
        self.system_prompt = prompt;
        self.rebuild_system_message();
    }

    fn rebuild_system_message(&mut self) {
        self.messages
            .retain(|message| message.role != ProviderRole::System);
        if let Some(prompt) = &self.system_prompt {
            if !prompt.trim().is_empty() {
                self.messages
                    .insert(0, ChatMessage::new(ProviderRole::System, prompt.clone()));
            }
        }
    }

    /// Starts a turn for `text`: interrupts any in-flight turn and appends the
    /// user message. Returns `true` if an in-flight turn was interrupted.
    pub fn start_turn(&mut self, text: String) -> bool {
        let interrupted = self.active_turn;
        self.token.cancel();
        self.token = CancellationToken::new();
        self.active_turn = true;
        self.messages
            .push(ChatMessage::new(ProviderRole::User, text));
        self.trim_history();
        interrupted
    }

    /// Cancels the in-flight turn (explicit `interrupt-signal`).
    pub fn cancel_turn(&mut self) -> bool {
        let was_active = self.active_turn;
        self.token.cancel();
        self.active_turn = false;
        was_active
    }

    /// Runs the provider call for the current turn and records the reply.
    pub async fn run_turn(&mut self) -> TurnOutcome {
        let Some(turn) = self.take_active_turn() else {
            return TurnOutcome::Cancelled;
        };
        let outcome = run_active_turn(self.provider.clone(), turn).await;
        if let TurnOutcome::Completed(response) = &outcome {
            self.record_reply(response.text.clone());
        }
        outcome
    }

    /// Moves the current in-flight turn out of the session so the provider
    /// call can be spawned; returns `None` when no turn is active.
    pub fn take_active_turn(&mut self) -> Option<ActiveTurn> {
        if !self.active_turn {
            return None;
        }
        self.active_turn = false;
        Some(ActiveTurn {
            messages: self.messages.clone(),
            token: self.token.clone(),
        })
    }

    /// Records an assistant reply (from a spawned [`run_active_turn`]).
    pub fn record_reply(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.messages
            .push(ChatMessage::new(ProviderRole::Assistant, text));
        self.trim_history();
    }

    /// Records the assistant's tool calls so the provider sees them in the
    /// next round of the agent loop.
    pub fn record_assistant_tool_calls(
        &mut self,
        tool_calls: Vec<crate::provider::ToolCallRequest>,
    ) {
        self.messages.push(ChatMessage {
            role: ProviderRole::Assistant,
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        });
        self.trim_history();
    }

    /// Appends one tool result message (role=Tool) after executing a call.
    pub fn append_tool_result(&mut self, tool_call_id: String, content: String) {
        self.messages.push(ChatMessage {
            role: ProviderRole::Tool,
            content,
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        });
        self.trim_history();
    }

    /// Marks a new provider round (agent loop follow-up) as active without
    /// appending a user message.
    pub fn start_tool_followup(&mut self) {
        self.token = CancellationToken::new();
        self.active_turn = true;
    }

    /// True while a provider call is running.
    pub fn is_turn_active(&self) -> bool {
        self.active_turn
    }

    /// Current history (including the leading system message when set).
    pub fn history(&self) -> &[ChatMessage] {
        &self.messages
    }

    fn trim_history(&mut self) {
        // Keep the system message (index 0 when present) and the most recent
        // `history_limit` non-system messages.
        let has_system = self
            .messages
            .first()
            .is_some_and(|message| message.role == ProviderRole::System);
        let start = usize::from(has_system);
        let non_system = self.messages.len() - start;
        if non_system > self.history_limit {
            let keep_from = start + (non_system - self.history_limit);
            self.messages.drain(start..keep_from);
        }
    }
}

/// Resolves the character's persona prompt from the legacy character files.
/// Returns `None` when the file is missing or has no persona prompt.
pub fn character_prompt(adapter: &LegacySettingsAdapter, file_name: &str) -> Option<String> {
    adapter.find_character_prompt(file_name)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;

    use super::*;
    use crate::provider::{ProviderConfig, ProviderKind};

    /// A mock OpenAI-compatible server that echoes a canned reply.
    async fn spawn_echo_provider(reply: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/chat/completions",
            post(move |_: Request<Body>| {
                let reply = reply;
                async move {
                    let body = format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{reply}\"}},\"index\":0}}]}}\n\n\
                         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\",\"index\":0}}]}}\n\n\
                         data: [DONE]\n\n"
                    );
                    axum::response::Response::builder()
                        .header("Content-Type", "text/event-stream")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });
        format!("http://{addr}")
    }

    fn provider(base_url: &str) -> std::sync::Arc<dyn ChatProvider> {
        std::sync::Arc::from(
            crate::provider::build_provider(&ProviderConfig {
                kind: ProviderKind::OpenAi,
                base_url: base_url.to_owned(),
                api_key: Some("sk-test".to_owned()),
                default_model: Some("test-model".to_owned()),
                timeout: Duration::from_secs(5),
            })
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn single_turn_returns_reply_and_records_history() {
        let base = spawn_echo_provider("Hi there!").await;
        let mut session = ChatSession::new(
            provider(&base),
            DEFAULT_HISTORY_LIMIT,
            Some("You are a helpful assistant.".to_owned()),
        );
        assert!(!session.start_turn("hello".to_owned()));
        let outcome = session.run_turn().await;
        let TurnOutcome::Completed(response) = outcome else {
            panic!("expected completed turn, got {outcome:?}");
        };
        assert_eq!(response.text, "Hi there!");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));

        let history = session.history();
        assert_eq!(history.len(), 3); // system + user + assistant
        assert_eq!(history[0].role, ProviderRole::System);
        assert_eq!(history[1].content, "hello");
        assert_eq!(history[2].content, "Hi there!");
        assert!(!session.is_turn_active());
    }

    #[tokio::test]
    async fn second_turn_interrupts_first_and_history_grows() {
        let base = spawn_echo_provider("ok").await;
        let mut session = ChatSession::new(provider(&base), DEFAULT_HISTORY_LIMIT, None);

        session.start_turn("first".to_owned());
        assert!(session.is_turn_active());
        // A new input while the turn is active interrupts it.
        assert!(session.start_turn("second".to_owned()));
        let TurnOutcome::Completed(_) = session.run_turn().await else {
            panic!("expected completed turn");
        };
        // The interrupted first turn produced no assistant reply, so the
        // history is user1 + user2 + assistant2 = 3 messages.
        assert_eq!(session.history().len(), 3);
        assert_eq!(session.history()[0].content, "first");
        assert_eq!(session.history()[1].content, "second");
        assert_eq!(session.history()[2].content, "ok");
    }

    #[tokio::test]
    async fn explicit_cancel_yields_cancelled_outcome() {
        let base = spawn_echo_provider("slow").await;
        let mut session = ChatSession::new(provider(&base), DEFAULT_HISTORY_LIMIT, None);

        session.start_turn("hello".to_owned());
        assert!(session.cancel_turn());
        let outcome = session.run_turn().await;
        assert!(matches!(outcome, TurnOutcome::Cancelled));
        // The user message stays in history (the turn never ran).
        assert_eq!(session.history().len(), 1);
    }

    #[tokio::test]
    async fn run_turn_without_active_turn_is_cancelled() {
        let base = spawn_echo_provider("nope").await;
        let mut session = ChatSession::new(provider(&base), DEFAULT_HISTORY_LIMIT, None);
        let outcome = session.run_turn().await;
        assert!(matches!(outcome, TurnOutcome::Cancelled));
    }

    #[tokio::test]
    async fn history_is_trimmed_to_limit() {
        let base = spawn_echo_provider("ok").await;
        let mut session = ChatSession::new(provider(&base), 4, None);
        for index in 0..6 {
            session.start_turn(format!("input-{index}"));
            let _ = session.run_turn().await;
        }
        // 4 most recent non-system messages: 2 user + 2 assistant.
        let history = session.history();
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].content, "input-4");
        assert_eq!(history[1].content, "ok");
        assert_eq!(history[2].content, "input-5");
        assert_eq!(history[3].content, "ok");
    }

    #[tokio::test]
    async fn character_prompt_switch_rebuilds_system_message() {
        let base = spawn_echo_provider("ok").await;
        let mut session = ChatSession::new(
            provider(&base),
            DEFAULT_HISTORY_LIMIT,
            Some("Original persona".to_owned()),
        );
        session.start_turn("hello".to_owned());
        let _ = session.run_turn().await;

        session.set_character_prompt(Some("New persona".to_owned()));
        let history = session.history();
        assert_eq!(history[0].role, ProviderRole::System);
        assert_eq!(history[0].content, "New persona");
        // History preserved: system + user + assistant.
        assert_eq!(history.len(), 3);

        session.set_character_prompt(None);
        assert_eq!(session.history()[0].role, ProviderRole::User);
    }

    #[test]
    fn history_trim_keeps_system_message_when_present() {
        // Exercise trim_history directly via many turns with a small limit.
        let base_url = "http://127.0.0.1:1"; // never called here
        let _ = base_url;
        // Use a provider that fails instantly to avoid network calls.
        struct BrokenProvider;
        #[async_trait::async_trait]
        impl ChatProvider for BrokenProvider {
            fn name(&self) -> &'static str {
                "broken"
            }
            async fn stream(
                &self,
                _request: &ProviderRequest,
                _token: &CancellationToken,
            ) -> Result<
                futures_util::stream::BoxStream<
                    'static,
                    Result<crate::provider::StreamChunk, ProviderError>,
                >,
                ProviderError,
            > {
                Err(ProviderError::Config("unused".to_owned()))
            }
        }
        let mut session = ChatSession::new(
            std::sync::Arc::new(BrokenProvider),
            3,
            Some("system".to_owned()),
        );
        for index in 0..5 {
            session.start_turn(format!("u{index}"));
            // start_turn trims; run_turn would fail, which is fine here.
        }
        let history = session.history();
        assert_eq!(history[0].role, ProviderRole::System);
        assert_eq!(history.len(), 4); // system + 3 most recent
    }
}
