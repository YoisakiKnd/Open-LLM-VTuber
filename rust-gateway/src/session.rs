//! Session supervision: the Rust-side view of the conversation lifecycle.
//!
//! This module is a *supervisory* layer: it observes the existing WebSocket
//! message flow (client -> gateway -> Python runtime and back) without
//! changing it, and maintains a structured view of the current conversation
//! (phase, current character, interrupt/turn counters, and a bounded
//! transcript). It is intentionally protocol-preserving: every message keeps
//! being forwarded exactly as before, so the Python runtime and legacy
//! clients remain fully compatible.
//!
//! The transcript and counters are the data foundation for the upcoming
//! native Rust conversation/provider work (M2/M3) and for the settings UI
//! migration (M4).

use std::collections::VecDeque;
use std::time::Instant;

use serde::Serialize;

/// The lifecycle phase of the current session as seen by the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    /// No input captured and no generation or TTS playback in flight.
    Idle,
    /// Audio or text input is being captured from the client.
    Listening,
    /// A conversation turn is being processed (LLM call in flight).
    Generating,
    /// Assistant audio is being played back to the client.
    Speaking,
}

/// The party that produced a transcript entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
}

/// A single bounded transcript entry.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    /// Free text of the entry when applicable (user text, assistant text,
    /// error message, or the target character file on a switch).
    pub text: Option<String>,
    /// The active character file name at the time the entry was recorded.
    pub character: Option<String>,
    /// Milliseconds since the supervisor was created.
    pub ts: u64,
}

/// Point-in-time snapshot of the session, served over HTTP.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub schema_version: u32,
    pub phase: SessionPhase,
    pub character: Option<String>,
    pub interrupts: u64,
    pub turns: u64,
    pub transcript: Vec<TranscriptEntry>,
    pub uptime_ms: u64,
}

/// Default upper bound for the transcript buffer.
pub const DEFAULT_MAX_TRANSCRIPT_ENTRIES: usize = 200;

/// Structured view of a client->gateway text message relevant to the
/// conversation lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSignal {
    pub message_type: String,
    pub text: Option<String>,
    pub file: Option<String>,
}

/// Structured view of a gateway->client (upstream) text message relevant to
/// the conversation lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamSignal {
    pub message_type: String,
    pub text: Option<String>,
}

/// Parses a client text message into a conversation signal, if any.
/// Unknown or malformed messages yield `None` and are simply forwarded.
pub fn parse_client_signal(text: &str) -> Option<ClientSignal> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let message_type = value.get("type")?.as_str()?.to_owned();
    Some(ClientSignal {
        message_type,
        text: value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        file: value
            .get("file")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

/// Parses a gateway->client (Python runtime) text message into a
/// conversation signal, if any.
pub fn parse_upstream_signal(text: &str) -> Option<UpstreamSignal> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let message_type = value.get("type")?.as_str()?.to_owned();
    Some(UpstreamSignal {
        message_type,
        text: value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

/// Supervises one gateway process's conversation lifecycle.
///
/// Shared across connections via `Arc<tokio::sync::Mutex<SessionSupervisor>>`.
/// The desktop app is single-user/single-connection, so a process-level view
/// matches its semantics; with concurrent connections the view is shared.
#[derive(Debug)]
pub struct SessionSupervisor {
    phase: SessionPhase,
    transcript: VecDeque<TranscriptEntry>,
    max_transcript_entries: usize,
    current_character: Option<String>,
    interrupts: u64,
    turns: u64,
    audio_open: bool,
    generation_in_flight: bool,
    audio_in_flight: bool,
    started_at: Instant,
}

impl SessionSupervisor {
    pub fn new(max_transcript_entries: usize) -> Self {
        Self {
            phase: SessionPhase::Idle,
            transcript: VecDeque::new(),
            max_transcript_entries: max_transcript_entries.max(1),
            current_character: None,
            interrupts: 0,
            turns: 0,
            audio_open: false,
            generation_in_flight: false,
            audio_in_flight: false,
            started_at: Instant::now(),
        }
    }

    /// Observes one client->gateway text message.
    pub fn observe_client_signal(&mut self, signal: &ClientSignal) {
        match signal.message_type.as_str() {
            "audio-start" | "mic-audio-start" => self.start_listening(),
            "audio-end" | "mic-audio-end" => self.complete_input(),
            "text-input" | "ai-speak-signal" => self.begin_turn(signal.text.clone()),
            "interrupt-signal" => self.interrupt(),
            "switch-config" => self.switch_character(signal.file.clone()),
            _ => {}
        }
    }

    /// Observes one gateway->client (Python runtime) text message.
    pub fn observe_upstream_signal(&mut self, signal: &UpstreamSignal) {
        match signal.message_type.as_str() {
            "full-text" => self.record_assistant_text(signal.text.clone()),
            "audio" => self.record_assistant_audio(),
            "interrupt-signal" => self.interrupt(),
            "error" => self.record_error(signal.text.clone()),
            "backend-synth-complete" => self.complete_synthesis(),
            _ => {}
        }
    }

    /// Returns the current snapshot.
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            schema_version: 1,
            phase: self.phase,
            character: self.current_character.clone(),
            interrupts: self.interrupts,
            turns: self.turns,
            transcript: self.transcript.iter().cloned().collect(),
            uptime_ms: self.started_at.elapsed().as_millis() as u64,
        }
    }

    /// Clears the transcript and counters, returning to Idle.
    pub fn reset(&mut self) {
        self.phase = SessionPhase::Idle;
        self.transcript.clear();
        self.interrupts = 0;
        self.turns = 0;
        self.audio_open = false;
        self.generation_in_flight = false;
        self.audio_in_flight = false;
        // The character is intentionally preserved across resets: it describes
        // the persona, not the transient conversation state.
    }

    fn start_listening(&mut self) {
        // The user starting to speak implicitly interrupts anything in flight.
        if self.generation_in_flight || self.audio_in_flight {
            self.interrupts += 1;
        }
        self.audio_open = true;
        self.generation_in_flight = false;
        self.audio_in_flight = false;
        self.phase = SessionPhase::Listening;
    }

    fn complete_input(&mut self) {
        if !self.audio_open {
            // Tolerate out-of-order or duplicated end markers: they do not
            // start a turn, they only close the capture window.
            self.audio_open = false;
            return;
        }
        self.audio_open = false;
        self.begin_turn(None);
    }

    fn begin_turn(&mut self, text: Option<String>) {
        if self.generation_in_flight || self.audio_in_flight {
            self.interrupts += 1;
        }
        self.generation_in_flight = true;
        self.audio_in_flight = false;
        self.phase = SessionPhase::Generating;
        self.turns += 1;
        self.push_entry(TranscriptRole::User, text, self.current_character.clone());
    }

    fn interrupt(&mut self) {
        // An upstream echo of a client interrupt arrives when nothing is in
        // flight; it must not double-count the client-side interrupt.
        if !self.generation_in_flight && !self.audio_in_flight && self.phase == SessionPhase::Idle {
            return;
        }
        self.interrupts += 1;
        self.generation_in_flight = false;
        self.audio_in_flight = false;
        self.audio_open = false;
        self.phase = SessionPhase::Idle;
    }

    fn switch_character(&mut self, file: Option<String>) {
        if let Some(file) = file {
            if self.current_character.as_deref() == Some(file.as_str()) {
                return;
            }
            self.current_character = Some(file.clone());
            self.push_entry(TranscriptRole::System, Some(file), None);
        }
    }

    fn record_assistant_text(&mut self, text: Option<String>) {
        self.push_entry(
            TranscriptRole::Assistant,
            text,
            self.current_character.clone(),
        );
    }

    fn record_assistant_audio(&mut self) {
        self.generation_in_flight = false;
        self.audio_in_flight = true;
        self.phase = SessionPhase::Speaking;
        self.push_entry(
            TranscriptRole::Assistant,
            None,
            self.current_character.clone(),
        );
    }

    fn complete_synthesis(&mut self) {
        self.audio_in_flight = false;
        if !self.generation_in_flight {
            self.phase = SessionPhase::Idle;
        }
    }

    fn record_error(&mut self, text: Option<String>) {
        self.generation_in_flight = false;
        self.audio_in_flight = false;
        self.audio_open = false;
        self.phase = SessionPhase::Idle;
        self.push_entry(TranscriptRole::System, text, self.current_character.clone());
    }

    fn push_entry(
        &mut self,
        role: TranscriptRole,
        text: Option<String>,
        character: Option<String>,
    ) {
        self.transcript.push_back(TranscriptEntry {
            role,
            text,
            character,
            ts: self.started_at.elapsed().as_millis() as u64,
        });
        while self.transcript.len() > self.max_transcript_entries {
            self.transcript.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(message_type: &str) -> ClientSignal {
        ClientSignal {
            message_type: message_type.to_owned(),
            text: None,
            file: None,
        }
    }

    fn upstream(message_type: &str) -> UpstreamSignal {
        UpstreamSignal {
            message_type: message_type.to_owned(),
            text: None,
        }
    }

    #[test]
    fn full_audio_flow_tracks_phases() {
        let mut s = SessionSupervisor::new(200);
        assert_eq!(s.snapshot().phase, SessionPhase::Idle);

        s.observe_client_signal(&client("mic-audio-start"));
        assert_eq!(s.snapshot().phase, SessionPhase::Listening);

        s.observe_client_signal(&client("mic-audio-end"));
        assert_eq!(s.snapshot().phase, SessionPhase::Generating);
        assert_eq!(s.snapshot().turns, 1);

        s.observe_upstream_signal(&upstream("full-text"));
        assert_eq!(s.snapshot().phase, SessionPhase::Generating);
        assert_eq!(s.snapshot().transcript[1].role, TranscriptRole::Assistant);

        s.observe_upstream_signal(&upstream("audio"));
        assert_eq!(s.snapshot().phase, SessionPhase::Speaking);

        s.observe_upstream_signal(&upstream("backend-synth-complete"));
        assert_eq!(s.snapshot().phase, SessionPhase::Idle);
    }

    #[test]
    fn protocol_v1_audio_start_end_also_drives_the_state_machine() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("audio-start"));
        assert_eq!(s.snapshot().phase, SessionPhase::Listening);
        s.observe_client_signal(&client("audio-end"));
        assert_eq!(s.snapshot().phase, SessionPhase::Generating);
        assert_eq!(s.snapshot().turns, 1);
    }

    #[test]
    fn text_input_starts_a_turn_with_its_text() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&ClientSignal {
            message_type: "text-input".to_owned(),
            text: Some("hello".to_owned()),
            file: None,
        });
        let snapshot = s.snapshot();
        assert_eq!(snapshot.phase, SessionPhase::Generating);
        assert_eq!(snapshot.transcript[0].role, TranscriptRole::User);
        assert_eq!(snapshot.transcript[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn input_while_generating_counts_as_interrupt() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("text-input"));
        assert_eq!(s.snapshot().interrupts, 0);
        s.observe_client_signal(&client("text-input"));
        assert_eq!(s.snapshot().interrupts, 1);
        assert_eq!(s.snapshot().turns, 2);
    }

    #[test]
    fn explicit_interrupt_is_counted_once_despite_upstream_echo() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("text-input"));
        assert_eq!(s.snapshot().phase, SessionPhase::Generating);

        s.observe_client_signal(&client("interrupt-signal"));
        assert_eq!(s.snapshot().interrupts, 1);
        assert_eq!(s.snapshot().phase, SessionPhase::Idle);

        // The Python runtime broadcasts the interrupt back to the client;
        // nothing is in flight, so this must not count again.
        s.observe_upstream_signal(&upstream("interrupt-signal"));
        assert_eq!(s.snapshot().interrupts, 1);
    }

    #[test]
    fn switch_character_records_system_entry_and_dedupes() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&ClientSignal {
            message_type: "switch-config".to_owned(),
            text: None,
            file: Some("mao_pro.yaml".to_owned()),
        });
        s.observe_client_signal(&ClientSignal {
            message_type: "switch-config".to_owned(),
            text: None,
            file: Some("mao_pro.yaml".to_owned()),
        });
        let snapshot = s.snapshot();
        assert_eq!(snapshot.character.as_deref(), Some("mao_pro.yaml"));
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(snapshot.transcript[0].role, TranscriptRole::System);
        assert_eq!(snapshot.transcript[0].text.as_deref(), Some("mao_pro.yaml"));
    }

    #[test]
    fn error_returns_to_idle() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("text-input"));
        assert_eq!(s.snapshot().phase, SessionPhase::Generating);
        s.observe_upstream_signal(&UpstreamSignal {
            message_type: "error".to_owned(),
            text: Some("boom".to_owned()),
        });
        let snapshot = s.snapshot();
        assert_eq!(snapshot.phase, SessionPhase::Idle);
        assert_eq!(
            snapshot.transcript.last().unwrap().role,
            TranscriptRole::System
        );
        assert_eq!(
            snapshot.transcript.last().unwrap().text.as_deref(),
            Some("boom")
        );
    }

    #[test]
    fn transcript_is_bounded() {
        let mut s = SessionSupervisor::new(3);
        for _ in 0..10 {
            s.observe_client_signal(&client("text-input"));
        }
        assert_eq!(s.snapshot().transcript.len(), 3);
        assert_eq!(s.snapshot().turns, 10);
    }

    #[test]
    fn out_of_order_audio_end_is_tolerated() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("mic-audio-end"));
        assert_eq!(s.snapshot().phase, SessionPhase::Idle);
        assert_eq!(s.snapshot().turns, 0);
    }

    #[test]
    fn reset_clears_transient_state_but_keeps_character() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&ClientSignal {
            message_type: "switch-config".to_owned(),
            text: None,
            file: Some("mao_pro.yaml".to_owned()),
        });
        s.observe_client_signal(&client("text-input"));
        s.observe_client_signal(&client("interrupt-signal"));
        s.reset();
        let snapshot = s.snapshot();
        assert_eq!(snapshot.phase, SessionPhase::Idle);
        assert_eq!(snapshot.turns, 0);
        assert_eq!(snapshot.interrupts, 0);
        assert!(snapshot.transcript.is_empty());
        assert_eq!(snapshot.character.as_deref(), Some("mao_pro.yaml"));
    }

    #[test]
    fn parse_signals_extract_expected_fields() {
        let signal = parse_client_signal(r#"{"type":"switch-config","file":"mao.yaml"}"#).unwrap();
        assert_eq!(signal.message_type, "switch-config");
        assert_eq!(signal.file.as_deref(), Some("mao.yaml"));
        assert!(parse_client_signal("not json").is_none());
        assert!(parse_client_signal(r#"{"text":"no type"}"#).is_none());
        let upstream = parse_upstream_signal(r#"{"type":"full-text","text":"hi"}"#).unwrap();
        assert_eq!(upstream.text.as_deref(), Some("hi"));
    }

    #[test]
    fn unknown_messages_are_ignored() {
        let mut s = SessionSupervisor::new(200);
        s.observe_client_signal(&client("heartbeat"));
        s.observe_client_signal(&client("fetch-configs"));
        s.observe_upstream_signal(&upstream("history-data"));
        assert_eq!(s.snapshot().phase, SessionPhase::Idle);
        assert!(s.snapshot().transcript.is_empty());
    }
}
