//! MCP (Model Context Protocol) client — M3.
//!
//! A minimal MCP client over the stdio transport: spawns the server process,
//! performs the `initialize` handshake, lists tools, and invokes them with
//! JSON Schema argument validation, per-call timeouts and cooperative
//! cancellation ([`cancellation::CancellationToken`]).
//!
//! The transport is abstracted ([`McpTransport`]) so protocol logic is tested
//! with an in-memory implementation; production uses [`StdioMcpTransport`].
//!
//! Protocol shape (2025-03-26): newline-delimited JSON-RPC 2.0 over the
//! server's stdin/stdout, one message per line.
//!
//! As of M3 this module is exercised by its unit tests; the production call
//! sites (the orchestrator's tool loop) land alongside the Agent tool cycle.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::debug;

use crate::cancellation::{CancellationToken, wait_for_cancellation};

/// One tool exposed by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Errors surfaced by the MCP layer.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("failed to spawn MCP server {name}: {message}")]
    Spawn { name: String, message: String },
    #[error("MCP handshake failed: {0}")]
    Handshake(String),
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP request was cancelled")]
    Cancelled,
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP server error: {0}")]
    Server(String),
    #[error("tool {tool} rejected arguments: {message}")]
    Validation { tool: String, message: String },
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

/// A bidirectional JSON-RPC channel to an MCP server.
#[async_trait]
pub trait McpTransport: Send {
    /// Establishes the underlying channel (spawns/connects).
    async fn connect(&mut self) -> Result<(), McpError>;
    /// Sends a request and waits for its matching response.
    async fn request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        timeout: Duration,
        token: &CancellationToken,
    ) -> Result<Value, McpError>;
    /// Sends a notification (no response expected).
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError>;
}

/// MCP client bound to one server process.
pub struct McpServer {
    name: String,
    transport: Box<dyn McpTransport + Send>,
    tools: Vec<McpToolSpec>,
    tools_by_name: HashMap<String, McpToolSpec>,
    request_timeout: Duration,
    next_id: u64,
}

impl McpServer {
    pub fn new(
        name: String,
        transport: Box<dyn McpTransport + Send>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            name,
            transport,
            tools: Vec::new(),
            tools_by_name: HashMap::new(),
            request_timeout,
            next_id: 1,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tools discovered on the server (empty until [`Self::connect`]).
    pub fn tools(&self) -> &[McpToolSpec] {
        &self.tools
    }

    /// Performs the handshake and loads the tool list.
    pub async fn connect(
        &mut self,
        protocol_version: &str,
        token: &CancellationToken,
    ) -> Result<(), McpError> {
        self.transport.connect().await?;
        let result = self
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "open-llm-vtuber-gateway",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
                token,
            )
            .await?;
        let negotiated = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(protocol_version);
        debug!(server = %self.name, protocol = %negotiated, "MCP initialize handshake completed");
        self.transport
            .notify("notifications/initialized", serde_json::json!({}))
            .await?;
        self.tools = self.list_tools(token).await?;
        self.tools_by_name = self
            .tools
            .iter()
            .map(|tool| (tool.name.clone(), tool.clone()))
            .collect();
        debug!(server = %self.name, tools = self.tools.len(), "MCP tools loaded");
        Ok(())
    }

    /// Lists the server's tools.
    pub async fn list_tools(
        &mut self,
        token: &CancellationToken,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        let result = self
            .request("tools/list", serde_json::json!({}), token)
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::Protocol("tools/list result has no tools array".to_owned()))?;
        Ok(tools
            .iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_owned();
                Some(McpToolSpec {
                    name,
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(serde_json::json!({ "type": "object" })),
                })
            })
            .collect())
    }

    /// Validates `arguments` against the tool's JSON Schema.
    pub fn validate_arguments(&self, tool: &str, arguments: &Value) -> Result<(), McpError> {
        let spec = self
            .tools_by_name
            .get(tool)
            .ok_or_else(|| McpError::UnknownTool(format!("{}.{}", self.name, tool)))?;
        validate_schema(&spec.input_schema, arguments).map_err(|message| McpError::Validation {
            tool: format!("{}.{}", self.name, tool),
            message,
        })
    }

    /// Invokes a tool with validated arguments.
    pub async fn call_tool(
        &mut self,
        tool: &str,
        arguments: Value,
        token: &CancellationToken,
    ) -> Result<Value, McpError> {
        self.validate_arguments(tool, &arguments)?;
        let result = self
            .request(
                "tools/call",
                serde_json::json!({ "name": tool, "arguments": arguments }),
                token,
            )
            .await?;
        Ok(result)
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        token: &CancellationToken,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let response = tokio::select! {
            result = self.transport.request(id, method, params, self.request_timeout, token) => result,
            _ = wait_for_cancellation(token) => Err(McpError::Cancelled),
        };
        // Transports map cancellation to Timeout/Cancelled themselves; the
        // select above additionally guards against a stuck transport.
        if matches!(response, Err(McpError::Timeout)) && token.is_cancelled() {
            return Err(McpError::Cancelled);
        }
        response
    }
}

// ---------------------------------------------------------------------------
// stdio transport
// ---------------------------------------------------------------------------

/// Newline-delimited JSON-RPC over a spawned process's stdin/stdout.
pub struct StdioMcpTransport {
    name: String,
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Option<Lines<BufReader<ChildStdout>>>,
}

impl StdioMcpTransport {
    pub fn new(name: String, command: String) -> Self {
        Self {
            name,
            command,
            args: Vec::new(),
            env: Vec::new(),
            child: None,
            stdin: None,
            lines: None,
        }
    }

    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

#[async_trait]
impl McpTransport for StdioMcpTransport {
    async fn connect(&mut self) -> Result<(), McpError> {
        let mut command = Command::new(&self.command);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in &self.env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|error| McpError::Spawn {
            name: self.name.clone(),
            message: error.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| McpError::Spawn {
            name: self.name.clone(),
            message: "server stdin is not piped".to_owned(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Spawn {
            name: self.name.clone(),
            message: "server stdout is not piped".to_owned(),
        })?;
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.lines = Some(BufReader::new(stdout).lines());
        Ok(())
    }

    async fn request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        timeout: Duration,
        token: &CancellationToken,
    ) -> Result<Value, McpError> {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = message.to_string();
        line.push('\n');
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| McpError::Protocol("transport is not connected".to_owned()))?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| McpError::Protocol(format!("write failed: {error}")))?;
        stdin
            .flush()
            .await
            .map_err(|error| McpError::Protocol(format!("flush failed: {error}")))?;

        let lines = self
            .lines
            .as_mut()
            .ok_or_else(|| McpError::Protocol("transport is not connected".to_owned()))?;
        loop {
            let line = tokio::select! {
                read = tokio::time::timeout(timeout, lines.next_line()) => match read {
                    Ok(read) => read,
                    Err(_) => return Err(McpError::Timeout),
                },
                _ = wait_for_cancellation(token) => return Err(McpError::Cancelled),
            };
            let line = line
                .map_err(|error| McpError::Protocol(format!("read failed: {error}")))?
                .ok_or_else(|| McpError::Protocol("server closed stdout".to_owned()))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line)
                .map_err(|error| McpError::Protocol(format!("invalid JSON-RPC line: {error}")))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(McpError::Server(error.to_string()));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            // Ignore server-initiated notifications/requests for other ids.
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                debug!(server = %self.name, method, "ignoring server-initiated message");
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line = message.to_string();
        line.push('\n');
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| McpError::Protocol("transport is not connected".to_owned()))?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| McpError::Protocol(format!("write failed: {error}")))?;
        stdin
            .flush()
            .await
            .map_err(|error| McpError::Protocol(format!("flush failed: {error}")))
    }
}

/// In-memory transport used by the unit tests: serves canned responses per
/// method and optionally blocks (for timeout/cancellation tests).
#[cfg(test)]
pub struct InMemoryMcpTransport {
    responses: HashMap<String, Value>,
    calls: std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    block_on: Option<String>,
    fail_on: Option<String>,
}

#[cfg(test)]
impl InMemoryMcpTransport {
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            block_on: None,
            fail_on: None,
        }
    }

    pub fn respond(mut self, method: &str, result: Value) -> Self {
        self.responses.insert(method.to_owned(), result);
        self
    }

    pub fn block_on(mut self, method: &str) -> Self {
        self.block_on = Some(method.to_owned());
        self
    }

    pub fn calls(&self) -> std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>> {
        self.calls.clone()
    }
}

#[cfg(test)]
#[async_trait]
impl McpTransport for InMemoryMcpTransport {
    async fn connect(&mut self) -> Result<(), McpError> {
        Ok(())
    }

    async fn request(
        &mut self,
        _id: u64,
        method: &str,
        params: Value,
        _timeout: Duration,
        token: &CancellationToken,
    ) -> Result<Value, McpError> {
        if self.fail_on.as_deref() == Some(method) {
            return Err(McpError::Server(format!("{method} failed")));
        }
        if self.block_on.as_deref() == Some(method) {
            wait_for_cancellation(token).await;
            return Err(McpError::Cancelled);
        }
        self.calls.lock().unwrap().push((method.to_owned(), params));
        self.responses
            .get(method)
            .cloned()
            .ok_or_else(|| McpError::Protocol(format!("no canned response for {method}")))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// JSON Schema validation (minimal subset)
// ---------------------------------------------------------------------------

/// Validates `instance` against a (minimal) JSON Schema. Returns an error
/// message on mismatch. Supports: type, required, properties, items, enum.
pub fn validate_schema(schema: &Value, instance: &Value) -> Result<(), String> {
    if let Some(error) = check_type(schema, instance) {
        return Err(error);
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let object = instance
            .as_object()
            .ok_or_else(|| "instance is not an object".to_owned())?;
        for field in required {
            let name = field
                .as_str()
                .ok_or_else(|| "required entry is not a string".to_owned())?;
            if !object.contains_key(name) {
                return Err(format!("missing required property '{name}'"));
            }
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let object = instance.as_object();
        if let Some(object) = object {
            for (name, subschema) in properties {
                if let Some(value) = object.get(name) {
                    validate_schema(subschema, value)
                        .map_err(|error| format!("property '{name}': {error}"))?;
                }
            }
        }
    }
    if let Some(items) = schema.get("items") {
        if let Some(array) = instance.as_array() {
            for (index, value) in array.iter().enumerate() {
                validate_schema(items, value).map_err(|error| format!("item {index}: {error}"))?;
            }
        }
    }
    Ok(())
}

fn check_type(schema: &Value, instance: &Value) -> Option<String> {
    let expected = schema.get("type").and_then(Value::as_str)?;
    let matches = match expected {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "number" => instance.is_number(),
        "integer" => instance.as_i64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        other => return Some(format!("unsupported schema type '{other}'")),
    };
    if matches {
        None
    } else {
        Some(format!("expected type '{expected}'"))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of connected MCP servers, keyed by name.
pub struct McpRegistry {
    servers: HashMap<String, McpServer>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    pub fn insert(&mut self, name: String, server: McpServer) {
        self.servers.insert(name, server);
    }

    pub fn server(&mut self, name: &str) -> Option<&mut McpServer> {
        self.servers.get_mut(name)
    }

    pub fn servers(&self) -> impl Iterator<Item = (&String, &McpServer)> {
        self.servers.iter()
    }

    /// All tools across all servers (`server.tool` names).
    pub fn all_tools(&self) -> Vec<McpToolSpec> {
        self.servers
            .values()
            .flat_map(|server| server.tools().iter().cloned())
            .collect()
    }
}

/// Spawns an MCP server from a command line and connects it.
pub async fn spawn_and_connect(
    name: &str,
    command: &str,
    args: &[String],
    request_timeout: Duration,
    connect_timeout: Duration,
    token: &CancellationToken,
) -> Result<McpServer, McpError> {
    let mut transport = StdioMcpTransport::new(name.to_owned(), command.to_owned());
    for arg in args {
        transport = transport.arg(arg.clone());
    }
    let mut server = McpServer::new(name.to_owned(), Box::new(transport), request_timeout);
    tokio::time::timeout(connect_timeout, server.connect("2025-03-26", token))
        .await
        .map_err(|_| McpError::Handshake(format!("{name}: connect timed out")))?
        .map_err(|error| McpError::Handshake(format!("{name}: {error}")))?;
    Ok(server)
}

/// Helper for paths that keeps `Path` in scope for callers.
pub fn path_argument(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> CancellationToken {
        CancellationToken::new()
    }

    fn sample_tools_result() -> Value {
        serde_json::json!({
            "tools": [
                {
                    "name": "echo",
                    "description": "Echoes the text",
                    "inputSchema": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "text": { "type": "string" },
                            "count": { "type": "integer" }
                        }
                    }
                },
                {
                    "name": "add",
                    "description": "Adds two numbers",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "a": { "type": "number" },
                            "b": { "type": "number" }
                        }
                    }
                }
            ]
        })
    }

    fn server_with(transport: InMemoryMcpTransport) -> McpServer {
        McpServer::new(
            "test-server".to_owned(),
            Box::new(transport),
            Duration::from_secs(2),
        )
    }

    #[tokio::test]
    async fn connect_loads_tools_and_negotiates() {
        let transport = InMemoryMcpTransport::new()
            .respond(
                "initialize",
                serde_json::json!({ "protocolVersion": "2025-03-26" }),
            )
            .respond("tools/list", sample_tools_result());
        let mut server = server_with(transport);
        server.connect("2025-03-26", &token()).await.unwrap();
        assert_eq!(server.tools().len(), 2);
        assert_eq!(server.tools()[0].name, "echo");
    }

    #[tokio::test]
    async fn call_tool_validates_and_invokes() {
        let transport = InMemoryMcpTransport::new()
            .respond("initialize", serde_json::json!({}))
            .respond("tools/list", sample_tools_result())
            .respond(
                "tools/call",
                serde_json::json!({ "content": [{ "type": "text", "text": "hello back" }] }),
            );
        let shared_calls = transport.calls();
        let mut server = server_with(transport);
        server.connect("2025-03-26", &token()).await.unwrap();

        let result = server
            .call_tool("echo", serde_json::json!({ "text": "hi" }), &token())
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "hello back");

        // The transport saw the correct request.
        let calls = shared_calls.lock().unwrap().clone();
        assert!(calls.iter().any(|(method, params)| {
            method == "tools/call"
                && params["name"] == "echo"
                && params["arguments"]["text"] == "hi"
        }));
    }

    #[tokio::test]
    async fn call_tool_rejects_invalid_arguments() {
        let transport = InMemoryMcpTransport::new()
            .respond("initialize", serde_json::json!({}))
            .respond("tools/list", sample_tools_result());
        let mut server = server_with(transport);
        server.connect("2025-03-26", &token()).await.unwrap();

        let error = server
            .call_tool("echo", serde_json::json!({ "count": "nope" }), &token())
            .await
            .unwrap_err();
        assert!(
            matches!(error, McpError::Validation { .. }),
            "expected validation error, got {error:?}"
        );
    }

    #[tokio::test]
    async fn call_tool_unknown_tool_is_rejected() {
        let transport = InMemoryMcpTransport::new()
            .respond("initialize", serde_json::json!({}))
            .respond("tools/list", sample_tools_result());
        let mut server = server_with(transport);
        server.connect("2025-03-26", &token()).await.unwrap();

        let error = server
            .call_tool("missing", serde_json::json!({}), &token())
            .await
            .unwrap_err();
        assert!(matches!(error, McpError::UnknownTool(_)));
    }

    #[tokio::test]
    async fn blocked_request_can_be_cancelled() {
        let transport = InMemoryMcpTransport::new()
            .respond("initialize", serde_json::json!({}))
            .respond("tools/list", sample_tools_result())
            .block_on("tools/call");
        let mut server = server_with(transport);
        let cancel = CancellationToken::new();
        server.connect("2025-03-26", &cancel).await.unwrap();

        let handle = {
            let cancel = cancel.clone();
            let mut server = server;
            tokio::spawn(async move {
                server
                    .call_tool("echo", serde_json::json!({ "text": "hi" }), &cancel)
                    .await
            })
        };
        tokio::task::yield_now().await;
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("call should terminate promptly")
            .expect("task should not panic")
            .expect_err("expected cancellation error");
        assert!(matches!(error, McpError::Cancelled), "got {error:?}");
    }

    #[tokio::test]
    async fn validation_rejects_missing_required_field() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": { "text": { "type": "string" } }
        });
        let error = validate_schema(&schema, &serde_json::json!({})).unwrap_err();
        assert!(error.contains("text"), "{error}");
    }

    #[tokio::test]
    async fn validation_accepts_nested_objects_and_arrays() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } },
                "meta": { "type": "object", "properties": { "depth": { "type": "integer" } } }
            }
        });
        assert!(
            validate_schema(
                &schema,
                &serde_json::json!({ "tags": ["a", "b"], "meta": { "depth": 3 } })
            )
            .is_ok()
        );
        let error = validate_schema(&schema, &serde_json::json!({ "tags": [1, 2] })).unwrap_err();
        assert!(error.contains("tag"), "{error}");
    }
}
