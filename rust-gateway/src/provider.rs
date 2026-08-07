//! Native chat providers (M2).
//!
//! A unified [`ChatProvider`] abstraction with three implementations:
//! OpenAI-compatible chat completions (OpenAI, DeepSeek, Moonshot, ...),
//! Anthropic Messages API, and Ollama's local `/api/chat`. All implementations
//! support streaming, tool-call deltas, timeouts and cooperative cancellation
//! via [`cancellation::CancellationToken`].
//!
//! Secrets never appear on the command line: API keys are read from the
//! environment (`OLV_PROVIDER_OPENAI_API_KEY` / `OLV_PROVIDER_ANTHROPIC_API_KEY`)
//! or injected via [`ProviderConfig`].
//!
//! As of M2 this module is exercised by its unit tests only; the production
//! call sites land with the conversation orchestrator (M2 completion) that
//! routes chat requests through the provider.
#![allow(dead_code)]

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{BoxStream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cancellation::{CancellationToken, wait_for_cancellation};

/// Role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole {
    System,
    User,
    Assistant,
    Tool,
}

impl FromStr for ProviderRole {
    type Err = ProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            other => Err(ProviderError::Unsupported(format!(
                "unknown provider role: {other}"
            ))),
        }
    }
}

impl ProviderRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// One chat message in the request. Content is plain text for M2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ProviderRole,
    pub content: String,
    /// Assistant tool calls (OpenAI-style), serialized per provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    /// Tool result association (role == Tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: ProviderRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

/// A tool invocation requested by the model (OpenAI-compatible shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// A tool the model may call (JSON Schema for `parameters`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Request passed to a [`ChatProvider`].
#[derive(Debug, Clone, Default)]
pub struct ProviderRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Vec<ToolSpec>,
}

/// A tool call emitted by the model.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Streaming accumulation buffer for `arguments`; never serialized.
    #[serde(skip)]
    pub arguments_buffer: String,
}

/// Final, accumulated provider response.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

/// Incremental streaming events.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    Done {
        finish_reason: Option<String>,
    },
}

/// Errors surfaced by providers.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider authentication failed: {0}")]
    Auth(String),
    #[error("provider rate limited: {0}")]
    RateLimited(String),
    #[error("provider returned an error: {0}")]
    Upstream(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
    #[error("request was cancelled")]
    Cancelled,
    #[error("invalid provider configuration: {0}")]
    Config(String),
    #[error("unsupported provider response: {0}")]
    Unsupported(String),
}

/// Provider kinds supported by this gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Ollama,
}

impl FromStr for ProviderKind {
    type Err = ProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "openai" | "openai-compatible" => Ok(Self::OpenAi),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            other => Err(ProviderError::Config(format!(
                "unsupported provider kind: {other}"
            ))),
        }
    }
}

/// Shared configuration for provider construction. API keys come from the
/// environment or the caller (settings domain, later), never from argv.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub timeout: Duration,
}

/// A chat completion provider.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Streams the completion. The returned stream terminates with
    /// `Ok(StreamChunk::Done)` or an error.
    async fn stream(
        &self,
        request: &ProviderRequest,
        token: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError>;

    /// Accumulates [`Self::stream`] into a final response. Default
    /// implementation: every provider only needs to implement streaming.
    async fn complete(
        &self,
        request: &ProviderRequest,
        token: &CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut stream = self.stream(request, token).await?;
        let mut response = ProviderResponse::default();
        loop {
            let next = tokio::select! {
                chunk = stream.next() => chunk,
                _ = wait_for_cancellation(token) => {
                    return Err(ProviderError::Cancelled);
                }
            };
            match next {
                Some(Ok(chunk)) => apply_chunk(chunk, &mut response),
                Some(Err(error)) => return Err(error),
                None => break,
            }
        }
        for call in &mut response.tool_calls {
            if call.arguments_buffer.is_empty() {
                continue;
            }
            call.arguments = serde_json::from_str(&call.arguments_buffer)
                .unwrap_or_else(|_| Value::String(call.arguments_buffer.clone()));
        }
        Ok(response)
    }
}

/// Applies one stream chunk onto the accumulated response.
fn apply_chunk(chunk: StreamChunk, response: &mut ProviderResponse) {
    match chunk {
        StreamChunk::TextDelta(text) => response.text.push_str(&text),
        StreamChunk::ToolCallDelta {
            index,
            id,
            name,
            arguments_delta,
        } => {
            while response.tool_calls.len() <= index {
                response.tool_calls.push(ToolCall::default());
            }
            let call = &mut response.tool_calls[index];
            if let Some(id) = id {
                call.id = id;
            }
            if let Some(name) = name {
                call.name = name;
            }
            call.arguments_buffer.push_str(&arguments_delta);
        }
        StreamChunk::Done { finish_reason } => {
            response.finish_reason = finish_reason;
        }
    }
}

/// Builds a provider for a given config. Panics on unsupported kinds are
/// avoided: [`ProviderKind::from_str`] already validates the kind.
pub fn build_provider(config: &ProviderConfig) -> Result<Box<dyn ChatProvider>, ProviderError> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(config.timeout)
        .build()
        .map_err(|error| ProviderError::Config(error.to_string()))?;
    match config.kind {
        ProviderKind::OpenAi => Ok(Box::new(OpenAiCompatibleProvider::new(config, client)?)),
        ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::new(config, client)?)),
        ProviderKind::Ollama => Ok(Box::new(OllamaProvider::new(config, client)?)),
    }
}

/// Appends a path segment to a base URL, tolerating a trailing slash.
fn join_url(base: &str, path: &str) -> Result<String, ProviderError> {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ProviderError::Config("base URL is empty".to_owned()));
    }
    Ok(format!("{trimmed}/{path}"))
}

/// Serializes a chat message for OpenAI-compatible APIs, including assistant
/// `tool_calls` and `role=tool` result messages.
fn openai_message_payload(message: &ChatMessage) -> Value {
    let mut payload = serde_json::json!({ "role": message.role.as_str() });
    match (&message.role, &message.tool_calls) {
        (ProviderRole::Tool, _) => {
            payload["tool_call_id"] =
                serde_json::json!(message.tool_call_id.as_deref().unwrap_or_default());
            payload["content"] = serde_json::json!(message.content);
        }
        (ProviderRole::Assistant, Some(tool_calls)) => {
            payload["content"] = Value::Null;
            payload["tool_calls"] = serde_json::json!(
                tool_calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            }
                        })
                    })
                    .collect::<Vec<_>>()
            );
        }
        _ => {
            payload["content"] = serde_json::json!(message.content);
        }
    }
    payload
}

/// Serializes a chat message for the Anthropic Messages API, mapping assistant
/// tool calls to `tool_use` blocks and tool results to `tool_result` blocks.
fn anthropic_message_payload(message: &ChatMessage) -> Value {
    let mut payload = serde_json::json!({ "role": message.role.as_str() });
    match (&message.role, &message.tool_calls) {
        (ProviderRole::Tool, _) => {
            payload["role"] = serde_json::json!("user");
            payload["content"] = serde_json::json!([{
                "type": "tool_result",
                "tool_use_id": message.tool_call_id.as_deref().unwrap_or_default(),
                "content": message.content,
            }]);
        }
        (ProviderRole::Assistant, Some(tool_calls)) => {
            let mut blocks = Vec::new();
            if !message.content.is_empty() {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": message.content,
                }));
            }
            for call in tool_calls {
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            payload["content"] = serde_json::json!(blocks);
        }
        _ => {
            payload["content"] = serde_json::json!(message.content);
        }
    }
    payload
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------

struct OpenAiCompatibleProvider {
    base_url: String,
    api_key: Option<String>,
    default_model: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    fn new(config: &ProviderConfig, client: reqwest::Client) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            default_model: config.default_model.clone(),
            timeout: config.timeout,
            client,
        })
    }

    fn request_payload(&self, request: &ProviderRequest) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(openai_message_payload)
            .collect();
        let mut payload = serde_json::json!({
            "model": request.model.as_deref().or(self.default_model.as_deref()).unwrap_or("gpt-4o-mini"),
            "messages": messages,
            "stream": true,
        });
        if let Some(temperature) = request.temperature {
            payload["temperature"] = serde_json::json!(temperature);
        }
        if let Some(max_tokens) = request.max_tokens {
            payload["max_tokens"] = serde_json::json!(max_tokens);
        }
        if !request.tools.is_empty() {
            payload["tools"] = serde_json::json!(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            }
                        })
                    })
                    .collect::<Vec<_>>()
            );
        }
        payload
    }
}

#[async_trait]
impl ChatProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
        token: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        let url = join_url(&self.base_url, "chat/completions")?;
        let mut builder = self
            .client
            .post(url)
            .timeout(self.timeout)
            .header("Content-Type", "application/json");
        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder
            .json(&self.request_payload(request))
            .send()
            .await
            .map_err(classify_network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_http_status(
                status,
                response.text().await.unwrap_or_default().trim(),
            ));
        }
        let stream = response.bytes_stream();
        Ok(Box::pin(openai_sse_stream(stream, token)))
    }
}

/// Parses OpenAI-style SSE lines (`data: {json}` / `data: [DONE]`).
fn openai_sse_stream<S>(
    stream: S,
    token: &CancellationToken,
) -> BoxStream<'static, Result<StreamChunk, ProviderError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut lines = sse_lines(stream).boxed();
    let token = token.clone();
    Box::pin(async_stream::try_stream! {
        let mut finished = false;
        while let Some(line) = next_or_cancel(&mut lines, &token).await {
            let line = line?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|error| ProviderError::Unsupported(format!("invalid SSE event: {error}")))?;
            let choices = event.get("choices").and_then(Value::as_array);
            let Some(Some(choice)) = choices.map(|choices| choices.first()) else {
                continue;
            };
            if let Some(delta) = choice.get("delta") {
                if let Some(text) = delta.get("content").and_then(Value::as_str) {
                    if !text.is_empty() {
                        yield StreamChunk::TextDelta(text.to_owned());
                    }
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in tool_calls {
                        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let function = call.get("function");
                        yield StreamChunk::ToolCallDelta {
                            index,
                            id: call.get("id").and_then(Value::as_str).map(str::to_owned),
                            name: function
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            arguments_delta: function
                                .and_then(|f| f.get("arguments"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        };
                    }
                }
            }
            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                finished = true;
                yield StreamChunk::Done {
                    finish_reason: Some(finish_reason.to_owned()),
                };
            }
        }
        if !finished {
            yield StreamChunk::Done {
                finish_reason: None,
            };
        }
    })
}

// ---------------------------------------------------------------------------
// Anthropic Messages
// ---------------------------------------------------------------------------

struct AnthropicProvider {
    base_url: String,
    api_key: Option<String>,
    default_model: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl AnthropicProvider {
    fn new(config: &ProviderConfig, client: reqwest::Client) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            default_model: config.default_model.clone(),
            timeout: config.timeout,
            client,
        })
    }

    fn request_payload(&self, request: &ProviderRequest) -> Value {
        let system: Vec<&str> = request
            .messages
            .iter()
            .filter(|message| message.role == ProviderRole::System)
            .map(|message| message.content.as_str())
            .collect();
        let messages: Vec<Value> = request
            .messages
            .iter()
            .filter(|message| message.role != ProviderRole::System)
            .map(anthropic_message_payload)
            .collect();
        let mut payload = serde_json::json!({
            "model": request.model.as_deref().or(self.default_model.as_deref()).unwrap_or("claude-sonnet-4-5"),
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "messages": messages,
            "stream": true,
        });
        if !system.is_empty() {
            payload["system"] = serde_json::json!(system.join("\n\n"));
        }
        if let Some(temperature) = request.temperature {
            payload["temperature"] = serde_json::json!(temperature);
        }
        if !request.tools.is_empty() {
            payload["tools"] = serde_json::json!(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.parameters,
                        })
                    })
                    .collect::<Vec<_>>()
            );
        }
        payload
    }
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
        token: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        let url = join_url(&self.base_url, "v1/messages")?;
        let mut builder = self
            .client
            .post(url)
            .timeout(self.timeout)
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01");
        if let Some(api_key) = &self.api_key {
            builder = builder.header("x-api-key", api_key);
        }
        let response = builder
            .json(&self.request_payload(request))
            .send()
            .await
            .map_err(classify_network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_http_status(
                status,
                response.text().await.unwrap_or_default().trim(),
            ));
        }
        let stream = response.bytes_stream();
        Ok(Box::pin(anthropic_sse_stream(stream, token)))
    }
}

/// Parses Anthropic SSE events (message_start / content_block_delta /
/// content_block_stop / message_delta / message_stop).
fn anthropic_sse_stream<S>(
    stream: S,
    token: &CancellationToken,
) -> BoxStream<'static, Result<StreamChunk, ProviderError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut lines = sse_lines(stream).boxed();
    let token = token.clone();
    Box::pin(async_stream::try_stream! {
        let mut finished = false;
        while let Some(line) = next_or_cancel(&mut lines, &token).await {
            let line = line?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|error| ProviderError::Unsupported(format!("invalid Anthropic SSE event: {error}")))?;
            match event.get("type").and_then(Value::as_str) {
                Some("content_block_delta") => {
                    let delta = event.get("delta");
                    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
                        if !text.is_empty() {
                            yield StreamChunk::TextDelta(text.to_owned());
                        }
                    }
                    if let Some(partial_json) = delta
                        .and_then(|d| d.get("partial_json"))
                        .and_then(Value::as_str)
                    {
                        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        yield StreamChunk::ToolCallDelta {
                            index,
                            id: None,
                            name: None,
                            arguments_delta: partial_json.to_owned(),
                        };
                    }
                }
                Some("content_block_start") => {
                    let content = event.get("content_block");
                    if let Some(name) = content
                        .and_then(|c| c.get("name"))
                        .and_then(Value::as_str)
                    {
                        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        yield StreamChunk::ToolCallDelta {
                            index,
                            id: content
                                .and_then(|c| c.get("id"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            name: Some(name.to_owned()),
                            arguments_delta: String::new(),
                        };
                    }
                }
                Some("message_delta") => {
                    if let Some(stop_reason) = event
                        .get("delta")
                        .and_then(|d| d.get("stop_reason"))
                        .and_then(Value::as_str)
                    {
                        finished = true;
                        yield StreamChunk::Done {
                            finish_reason: Some(stop_reason.to_owned()),
                        };
                    }
                }
                Some("message_stop") if !finished => {
                    yield StreamChunk::Done {
                        finish_reason: None,
                    };
                }
                _ => {}
            }
        }
        if !finished {
            yield StreamChunk::Done {
                finish_reason: None,
            };
        }
    })
}

// ---------------------------------------------------------------------------
// Ollama (local, NDJSON)
// ---------------------------------------------------------------------------

struct OllamaProvider {
    base_url: String,
    default_model: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl OllamaProvider {
    fn new(config: &ProviderConfig, client: reqwest::Client) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: config.base_url.clone(),
            default_model: config.default_model.clone(),
            timeout: config.timeout,
            client,
        })
    }

    fn request_payload(&self, request: &ProviderRequest) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(openai_message_payload)
            .collect();
        let mut payload = serde_json::json!({
            "model": request.model.as_deref().or(self.default_model.as_deref()).unwrap_or("llama3.2"),
            "messages": messages,
            "stream": true,
        });
        if let Some(temperature) = request.temperature {
            payload["options"] = serde_json::json!({ "temperature": temperature });
        }
        if !request.tools.is_empty() {
            payload["tools"] = serde_json::json!(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            }
                        })
                    })
                    .collect::<Vec<_>>()
            );
        }
        payload
    }
}

#[async_trait]
impl ChatProvider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
        token: &CancellationToken,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        let url = join_url(&self.base_url, "api/chat")?;
        let response = self
            .client
            .post(url)
            .timeout(self.timeout)
            .header("Content-Type", "application/json")
            .json(&self.request_payload(request))
            .send()
            .await
            .map_err(classify_network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_http_status(
                status,
                response.text().await.unwrap_or_default().trim(),
            ));
        }
        let stream = response.bytes_stream();
        Ok(Box::pin(ollama_ndjson_stream(stream, token)))
    }
}

/// Parses Ollama NDJSON chat events (`{"message":{"content":...},"done":true}`).
fn ollama_ndjson_stream<S>(
    stream: S,
    token: &CancellationToken,
) -> BoxStream<'static, Result<StreamChunk, ProviderError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut lines = ndjson_lines(stream).boxed();
    let token = token.clone();
    Box::pin(async_stream::try_stream! {
        while let Some(line) = next_or_cancel(&mut lines, &token).await {
            let line = line?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|error| ProviderError::Unsupported(format!("invalid Ollama event: {error}")))?;
            if let Some(text) = event
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
            {
                if !text.is_empty() {
                    yield StreamChunk::TextDelta(text.to_owned());
                }
            }
            if event.get("done").and_then(Value::as_bool).unwrap_or(false) {
                let reason = event.get("done_reason").and_then(Value::as_str).map(str::to_owned);
                yield StreamChunk::Done {
                    finish_reason: reason,
                };
                return;
            }
        }
        yield StreamChunk::Done {
            finish_reason: None,
        };
    })
}

// ---------------------------------------------------------------------------
// Shared SSE/NDJSON line splitting and cancellation
// ---------------------------------------------------------------------------

/// Splits an SSE byte stream into `data:` payload lines.
fn sse_lines<S>(stream: S) -> impl Stream<Item = Result<String, ProviderError>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = Box::pin(stream);
    async_stream::try_stream! {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ProviderError::Network(error.to_string()))?;
            buffer.extend_from_slice(&chunk);
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    yield data.to_owned();
                }
            }
        }
    }
}

/// Splits an NDJSON byte stream into JSON lines.
fn ndjson_lines<S>(stream: S) -> impl Stream<Item = Result<String, ProviderError>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = Box::pin(stream);
    async_stream::try_stream! {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ProviderError::Network(error.to_string()))?;
            buffer.extend_from_slice(&chunk);
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if !line.is_empty() {
                    yield line.to_owned();
                }
            }
        }
    }
}

/// Next line, honouring cancellation: returns `None` when cancelled.
async fn next_or_cancel<S, T>(
    stream: &mut S,
    token: &CancellationToken,
) -> Option<Result<T, ProviderError>>
where
    S: Stream<Item = Result<T, ProviderError>> + Unpin,
{
    tokio::select! {
        line = stream.next() => line,
        _ = wait_for_cancellation(token) => Some(Err(ProviderError::Cancelled)),
    }
}

fn classify_network_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else if error.is_connect() {
        ProviderError::Network(format!("connection failed: {error}"))
    } else {
        ProviderError::Network(error.to_string())
    }
}

fn classify_http_status(status: reqwest::StatusCode, body: &str) -> ProviderError {
    let detail = if body.is_empty() {
        status.to_string()
    } else {
        format!("{status}: {body}")
    };
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(detail),
        429 => ProviderError::RateLimited(detail),
        _ => ProviderError::Upstream(detail),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode};
    use axum::routing::post;
    use tokio::sync::Notify;

    use super::*;

    fn test_request() -> ProviderRequest {
        ProviderRequest {
            messages: vec![
                ChatMessage::new(ProviderRole::System, "You are a helpful assistant."),
                ChatMessage::new(ProviderRole::User, "Hello!"),
            ],
            ..Default::default()
        }
    }

    fn test_token() -> CancellationToken {
        CancellationToken::new()
    }

    /// Spawns an axum mock server and returns its base URL.
    async fn spawn_mock(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });
        format!("http://{addr}")
    }

    fn sse_body(lines: &[&str]) -> Response<Body> {
        let body: String = lines
            .iter()
            .map(|line| format!("data: {line}\n\n"))
            .collect();
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    fn sse_done() -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from("data: [DONE]\n\n"))
            .unwrap()
    }

    fn openai_config(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            kind: ProviderKind::OpenAi,
            base_url: base_url.to_owned(),
            api_key: Some("sk-test".to_owned()),
            default_model: Some("test-model".to_owned()),
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn openai_stream_accumulates_text_and_finish_reason() {
        let app = Router::new().route(
            "/chat/completions",
            post(|request: Request<Body>| async move {
                // Validate the request body shape.
                let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let payload: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(payload["model"], "test-model");
                assert_eq!(payload["stream"], true);
                assert_eq!(payload["messages"][1]["content"], "Hello!");
                assert_eq!(payload["messages"][0]["role"], "system");
                sse_body(&[
                    r#"{"id":"1","choices":[{"delta":{"role":"assistant","content":"Hel"},"index":0}]}"#,
                    r#"{"id":"1","choices":[{"delta":{"content":"lo"},"index":0}]}"#,
                    r#"{"id":"1","choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
                ])
            }),
        );
        let base = spawn_mock(app).await;
        let provider = build_provider(&openai_config(&base)).unwrap();

        let response = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap();
        assert_eq!(response.text, "Hello");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn openai_stream_collects_tool_call_deltas() {
        let app = Router::new().route(
            "/chat/completions",
            post(|_: Request<Body>| async {
                sse_body(&[
                    r#"{"id":"1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":"{\"q\":"}}]},"index":0}]}"#,
                    r#"{"id":"1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"weather\"}"}}]},"index":0}]}"#,
                    r#"{"id":"1","choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#,
                ])
            }),
        );
        let base = spawn_mock(app).await;
        let provider = build_provider(&openai_config(&base)).unwrap();

        let request = ProviderRequest {
            tools: vec![ToolSpec {
                name: "search".to_owned(),
                description: "Search the web".to_owned(),
                parameters: serde_json::json!({ "type": "object" }),
            }],
            ..test_request()
        };
        let response = provider.complete(&request, &test_token()).await.unwrap();
        assert_eq!(response.text, "");
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].name, "search");
        // The accumulated JSON string is exposed on `arguments` after parsing.
        assert_eq!(
            response.tool_calls[0].arguments,
            serde_json::json!({ "q": "weather" })
        );
    }

    #[tokio::test]
    async fn openai_auth_error_is_classified() {
        let app = Router::new().route(
            "/chat/completions",
            post(|_: Request<Body>| async {
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Body::from(r#"{"error":{"message":"bad key"}}"#))
                    .unwrap()
            }),
        );
        let base = spawn_mock(app).await;
        let provider = build_provider(&openai_config(&base)).unwrap();

        let error = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Auth(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn openai_rate_limit_is_classified() {
        let app = Router::new().route(
            "/chat/completions",
            post(|_: Request<Body>| async {
                Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .body(Body::from("slow down"))
                    .unwrap()
            }),
        );
        let base = spawn_mock(app).await;
        let provider = build_provider(&openai_config(&base)).unwrap();

        let error = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProviderError::RateLimited(_)),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn anthropic_stream_accumulates_text_deltas() {
        let app = Router::new().route(
            "/v1/messages",
            post(|request: Request<Body>| async move {
                let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let payload: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(payload["model"], "test-model");
                assert_eq!(payload["system"], "You are a helpful assistant.");
                assert_eq!(payload["messages"][0]["content"], "Hello!");
                sse_body(&[
                    r#"{"type":"message_start","message":{"id":"m1"}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi "}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"there"}}"#,
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
                    r#"{"type":"message_stop"}"#,
                ])
            }),
        );
        let base = spawn_mock(app).await;
        let config = ProviderConfig {
            kind: ProviderKind::Anthropic,
            base_url: base.clone(),
            api_key: Some("sk-ant-test".to_owned()),
            default_model: Some("test-model".to_owned()),
            timeout: Duration::from_secs(5),
        };
        let provider = build_provider(&config).unwrap();

        let response = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap();
        assert_eq!(response.text, "Hi there");
        assert_eq!(response.finish_reason.as_deref(), Some("end_turn"));
    }

    #[tokio::test]
    async fn ollama_stream_accumulates_ndjson() {
        let app = Router::new().route(
            "/api/chat",
            post(|request: Request<Body>| async move {
                let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                    .await
                    .unwrap();
                let payload: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(payload["model"], "test-model");
                let ndjson = "{\"message\":{\"role\":\"assistant\",\"content\":\"Bon\"},\"done\":false}\n{\"message\":{\"role\":\"assistant\",\"content\":\"jour\"},\"done\":false}\n{\"done\":true,\"done_reason\":\"stop\"}\n";
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/x-ndjson")
                    .body(Body::from(ndjson))
                    .unwrap()
            }),
        );
        let base = spawn_mock(app).await;
        let config = ProviderConfig {
            kind: ProviderKind::Ollama,
            base_url: base,
            api_key: None,
            default_model: Some("test-model".to_owned()),
            timeout: Duration::from_secs(5),
        };
        let provider = build_provider(&config).unwrap();

        let response = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap();
        assert_eq!(response.text, "Bonjour");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn cancellation_stops_streaming_early() {
        let started = Arc::new(Notify::new());
        let started_2 = started.clone();
        let app = Router::new().route(
            "/chat/completions",
            post(move |_: Request<Body>| async move {
                started_2.notify_one();
                // A stream that never finishes unless the client disconnects.
                let stream = futures_util::stream::unfold((), |()| async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Some((
                        Ok::<Bytes, std::io::Error>(Bytes::from(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"index\":0}]}\n\n",
                        )),
                        (),
                    ))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }),
        );
        let base = spawn_mock(app).await;
        let provider = build_provider(&openai_config(&base)).unwrap();

        let token = CancellationToken::new();
        let mut stream = provider.stream(&test_request(), &token).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("mock should receive the request");

        // Cancel while the stream is still producing chunks.
        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("stream should terminate promptly after cancellation");
        assert!(matches!(result, Some(Err(ProviderError::Cancelled))));
    }

    #[tokio::test]
    async fn timeout_is_classified() {
        let app = Router::new().route(
            "/chat/completions",
            post(|_: Request<Body>| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                sse_done()
            }),
        );
        let base = spawn_mock(app).await;
        let config = ProviderConfig {
            timeout: Duration::from_millis(50),
            ..openai_config(&base)
        };
        let provider = build_provider(&config).unwrap();

        let error = provider
            .complete(&test_request(), &test_token())
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Timeout), "got {error:?}");
    }

    #[tokio::test]
    async fn unsupported_kind_is_rejected() {
        let error = "unknown".parse::<ProviderKind>().unwrap_err();
        assert!(matches!(error, ProviderError::Config(_)));
    }

    #[tokio::test]
    async fn join_url_handles_trailing_slashes() {
        assert_eq!(
            join_url("https://api.example.com/", "chat/completions").unwrap(),
            "https://api.example.com/chat/completions"
        );
        assert!(join_url("", "chat/completions").is_err());
    }
}
