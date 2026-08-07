mod cancellation;
mod conversation;
mod legacy_settings;
mod mcp;
mod provider;
mod session;
mod settings;

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fs, io,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request, State, WebSocketUpgrade, ws::WebSocket},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, ORIGIN, RETRY_AFTER},
    },
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, patch, post},
};
use clap::Parser;
use futures_util::{SinkExt, Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc},
};
use tokio_tungstenite::{connect_async, tungstenite};
use tower_http::{
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::{Instrument, info, info_span, warn};
use url::Url;
use uuid::Uuid;

/// Whether the gateway forwards conversation messages to the Python runtime
/// (`proxy`, the default) or drives them through the native Rust provider
/// (`native`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ChatMode {
    Proxy,
    Native,
}

#[derive(Clone, Debug, Parser)]
#[command(version, about)]
struct Config {
    #[arg(long, env = "OLV_GATEWAY_LISTEN", default_value = "127.0.0.1:12394")]
    listen: SocketAddr,

    #[arg(
        long,
        env = "OLV_PYTHON_WS_URL",
        default_value = "ws://127.0.0.1:12393/internal/v1/session-ws"
    )]
    python_ws_url: Url,

    #[arg(long, env = "OLV_GATEWAY_MAX_CONNECTIONS", default_value_t = 64)]
    max_connections: usize,

    #[arg(long, env = "OLV_GATEWAY_MAX_CONNECTIONS_PER_IP", default_value_t = 8)]
    max_connections_per_ip: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_MAX_CONNECTION_ATTEMPTS_PER_MINUTE",
        default_value_t = 60
    )]
    max_connection_attempts_per_minute: u32,

    #[arg(
        long,
        env = "OLV_GATEWAY_MAX_MESSAGE_BYTES",
        default_value_t = 2 * 1024 * 1024
    )]
    max_message_bytes: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_MAX_HTTP_BODY_BYTES",
        default_value_t = 32 * 1024 * 1024
    )]
    max_http_body_bytes: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_HTTP_UPLOAD_IDLE_TIMEOUT_MS",
        default_value_t = 15_000
    )]
    http_upload_idle_timeout_ms: u64,

    #[arg(
        long,
        env = "OLV_GATEWAY_HTTP_RESPONSE_IDLE_TIMEOUT_MS",
        default_value_t = 60_000
    )]
    http_response_idle_timeout_ms: u64,

    #[arg(long, env = "OLV_GATEWAY_CONNECT_TIMEOUT_MS", default_value_t = 5000)]
    connect_timeout_ms: u64,

    #[arg(long, env = "OLV_GATEWAY_SESSION_QUEUE_CAPACITY", default_value_t = 32)]
    session_queue_capacity: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_MAX_TRANSCRIPT_ENTRIES",
        default_value_t = session::DEFAULT_MAX_TRANSCRIPT_ENTRIES
    )]
    max_transcript_entries: usize,

    #[arg(long, env = "OLV_GATEWAY_CHAT_MODE", default_value = "proxy")]
    chat_mode: ChatMode,

    #[arg(long, env = "OLV_GATEWAY_CHAT_PROVIDER", default_value = "openai")]
    chat_provider: provider::ProviderKind,

    #[arg(long, env = "OLV_GATEWAY_CHAT_BASE_URL")]
    chat_base_url: Option<String>,

    #[arg(long, env = "OLV_GATEWAY_CHAT_MODEL")]
    chat_model: Option<String>,

    #[arg(long, env = "OLV_GATEWAY_CHAT_SYSTEM_PROMPT")]
    chat_system_prompt: Option<String>,

    #[arg(
        long,
        env = "OLV_GATEWAY_CHAT_HISTORY_MESSAGES",
        default_value_t = conversation::DEFAULT_HISTORY_LIMIT
    )]
    chat_history_messages: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_CHAT_PROVIDER_TIMEOUT_MS",
        default_value_t = 60_000
    )]
    chat_provider_timeout_ms: u64,

    #[arg(long, env = "OLV_GATEWAY_MCP_SERVERS", value_delimiter = ';')]
    mcp_servers: Vec<String>,

    #[arg(long, env = "OLV_GATEWAY_MCP_TIMEOUT_MS", default_value_t = 15_000)]
    mcp_timeout_ms: u64,

    #[arg(long, env = "OLV_GATEWAY_MAX_TOOL_ROUNDS", default_value_t = 4)]
    max_tool_rounds: usize,

    #[arg(
        long,
        env = "OLV_GATEWAY_HTTP_REQUESTS_PER_MINUTE_PER_IP",
        default_value_t = 0
    )]
    http_requests_per_minute_per_ip: u32,

    #[arg(long, env = "OLV_GATEWAY_MAX_CONCURRENT_HTTP", default_value_t = 0)]
    max_concurrent_http: usize,

    /// Permit WebSocket sessions to start without the Python runtime
    /// (native chat mode serves conversations entirely from Rust).
    #[arg(long, env = "OLV_GATEWAY_ALLOW_MISSING_PYTHON")]
    allow_missing_python: bool,

    #[arg(
        long,
        env = "OLV_GATEWAY_MODEL_DICT_FILE",
        default_value = "model_dict.json"
    )]
    model_dict_file: PathBuf,

    #[arg(long, env = "OLV_GATEWAY_MAX_AUDIO_SECONDS", default_value_t = 120)]
    max_audio_seconds: usize,

    #[arg(long, env = "OLV_GATEWAY_VAD_RMS_THRESHOLD", default_value_t = 0.015)]
    vad_rms_threshold: f32,

    #[arg(long, env = "OLV_GATEWAY_VAD_FRAME_SAMPLES", default_value_t = 512)]
    vad_frame_samples: usize,

    #[arg(long, env = "OLV_GATEWAY_VAD_START_FRAMES", default_value_t = 3)]
    vad_start_frames: usize,

    #[arg(long, env = "OLV_GATEWAY_VAD_END_FRAMES", default_value_t = 24)]
    vad_end_frames: usize,

    #[arg(long, env = "OLV_GATEWAY_VAD_PRE_ROLL_FRAMES", default_value_t = 10)]
    vad_pre_roll_frames: usize,

    #[arg(long, env = "OLV_GATEWAY_FRONTEND_DIR", default_value = "frontend")]
    frontend_dir: PathBuf,

    #[arg(long, env = "OLV_GATEWAY_CACHE_DIR", default_value = "cache")]
    cache_dir: PathBuf,

    #[arg(
        long,
        env = "OLV_GATEWAY_LIVE2D_MODELS_DIR",
        default_value = "live2d-models"
    )]
    live2d_models_dir: PathBuf,

    #[arg(
        long,
        env = "OLV_GATEWAY_BACKGROUNDS_DIR",
        default_value = "backgrounds"
    )]
    backgrounds_dir: PathBuf,

    #[arg(long, env = "OLV_GATEWAY_AVATARS_DIR", default_value = "avatars")]
    avatars_dir: PathBuf,

    #[arg(long, env = "OLV_GATEWAY_WEB_TOOL_DIR", default_value = "web_tool")]
    web_tool_dir: PathBuf,

    #[arg(
        long,
        env = "OLV_GATEWAY_LEGACY_CONFIG_FILE",
        default_value = "conf.yaml"
    )]
    legacy_config_file: PathBuf,

    #[arg(
        long,
        env = "OLV_GATEWAY_LEGACY_CHARACTERS_DIR",
        default_value = "characters"
    )]
    legacy_characters_dir: PathBuf,

    #[arg(
        long,
        env = "OLV_GATEWAY_SETTINGS_FILE",
        default_value = ".olv/settings.v1.json"
    )]
    settings_file: PathBuf,

    #[arg(long, value_name = "PATH")]
    export_settings_types: Option<PathBuf>,

    #[arg(
        long,
        env = "OLV_GATEWAY_ALLOWED_ORIGINS",
        value_delimiter = ',',
        default_value = "http://localhost:12394,http://127.0.0.1:12394"
    )]
    allowed_origins: Vec<HeaderValue>,
}

impl Config {
    fn validate(&self) -> Result<()> {
        if self.max_connections == 0
            || self.max_connections_per_ip == 0
            || self.max_connection_attempts_per_minute == 0
            || self.max_message_bytes == 0
            || self.max_http_body_bytes == 0
            || self.http_upload_idle_timeout_ms == 0
            || self.http_response_idle_timeout_ms == 0
            || self.session_queue_capacity == 0
            || self.max_audio_seconds == 0
        {
            bail!("connection, size, queue, and audio limits must be greater than zero");
        }
        if self.settings_file.as_os_str().is_empty()
            || self.legacy_config_file.as_os_str().is_empty()
            || self.legacy_characters_dir.as_os_str().is_empty()
        {
            bail!("settings and legacy file paths must not be empty");
        }
        if self.max_connections_per_ip > self.max_connections {
            bail!("per-IP connection limit cannot exceed the global connection limit");
        }
        if !self.vad_rms_threshold.is_finite()
            || self.vad_rms_threshold <= 0.0
            || self.vad_rms_threshold > 1.0
        {
            bail!("VAD RMS threshold must be greater than zero and at most one");
        }
        if self.vad_frame_samples == 0 || self.vad_start_frames == 0 || self.vad_end_frames == 0 {
            bail!("VAD frame, start, and end counts must be greater than zero");
        }
        if self.chat_mode == ChatMode::Native
            && (self.chat_history_messages == 0 || self.chat_provider_timeout_ms == 0)
        {
            bail!("chat history messages and provider timeout must be greater than zero");
        }
        if self.mcp_timeout_ms == 0 {
            bail!("MCP request timeout must be greater than zero");
        }
        Ok(())
    }
}

#[derive(Default)]
/// Fixed-window per-IP HTTP request limiter. A limit of zero disables it.
struct HttpRateLimiter {
    limit_per_minute: u32,
    windows: Mutex<HashMap<std::net::IpAddr, RateWindow>>,
}

#[derive(Clone, Copy)]
struct RateWindow {
    started_at: Instant,
    count: u32,
}

impl HttpRateLimiter {
    fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` when the request may proceed.
    fn try_acquire(&self, peer: std::net::IpAddr) -> bool {
        if self.limit_per_minute == 0 {
            return true;
        }
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let window = windows.entry(peer).or_insert(RateWindow {
            started_at: now,
            count: 0,
        });
        if now.duration_since(window.started_at) >= Duration::from_secs(60) {
            *window = RateWindow {
                started_at: now,
                count: 0,
            };
        }
        if window.count >= self.limit_per_minute {
            return false;
        }
        window.count += 1;
        true
    }
}

/// Paths that remain accessible without an auth token (browser assets and
/// health probes). The WebSocket endpoint is also public because browsers
/// cannot attach `Authorization` headers to a `WebSocket` handshake; token
/// protection targets management and proxy endpoints.
fn is_public_path(path: &str) -> bool {
    path == "/"
        || path == "/healthz"
        || path == "/favicon.ico"
        || path.starts_with("/assets/")
        || path.starts_with("/libs/")
        || path.starts_with("/cache/")
        || path.starts_with("/live2d-models/")
        || path.starts_with("/bg/")
        || path.starts_with("/avatars/")
        || path.starts_with("/web-tool/")
        || path.starts_with("/client-ws")
}

/// Optional bearer-token guard for management and proxy endpoints.
async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(token) = &state.auth_token {
        let path = request.uri().path();
        if !is_public_path(path) {
            let authorized = request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == format!("Bearer {token}"));
            if !authorized {
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
        }
    }
    next.run(request).await
}

/// Fixed-window per-IP HTTP request rate limiting. The peer address is read
/// from the request extensions (populated by
/// `into_make_service_with_connect_info`); requests without it (e.g. direct
/// router tests) are not rate limited.
async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.ip());
    if let Some(peer_ip) = peer_ip {
        if !state.http_rate_limiter.try_acquire(peer_ip) {
            return (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response();
        }
    }
    next.run(request).await
}

/// Bounded HTTP concurrency (uploads/proxy requests).
async fn concurrency_middleware(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(semaphore) = &state.http_concurrency {
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "server busy").into_response();
            }
        };
        let response = next.run(request).await;
        drop(permit);
        return response;
    }
    next.run(request).await
}
struct ChatRuntime {
    provider: Arc<dyn provider::ChatProvider>,
    legacy_settings: Arc<legacy_settings::LegacySettingsAdapter>,
    history_limit: usize,
    system_prompt: Option<String>,
    mcp: Option<Arc<tokio::sync::Mutex<mcp::McpRegistry>>>,
    max_tool_rounds: usize,
}

/// Initialization payload served by the gateway when no Python runtime is
/// present: the frontend requests these on connect and expects immediate
/// answers (model info, character list, background list).
#[derive(Clone)]
struct InitRuntimeData {
    model_info: serde_json::Value,
    conf_name: String,
    conf_uid: String,
    config_files: Vec<String>,
    background_files: Vec<String>,
}

/// Builds the init payload from the model dictionary (fallback: first model
/// found under `live2d_models_dir`), the legacy default character, the
/// characters directory, and the backgrounds directory.
fn build_init_runtime_data(config: &Config) -> InitRuntimeData {
    let model_info = load_model_info(&config.model_dict_file, &config.live2d_models_dir);
    let (conf_name, conf_uid) = load_default_character(&config.legacy_config_file);
    InitRuntimeData {
        model_info,
        conf_name,
        conf_uid,
        config_files: yaml_file_names(&config.legacy_characters_dir),
        background_files: image_file_names(&config.backgrounds_dir),
    }
}

fn load_model_info(model_dict_file: &Path, live2d_models_dir: &Path) -> serde_json::Value {
    if let Ok(contents) = fs::read_to_string(model_dict_file) {
        if let Ok(models) = serde_json::from_str::<Vec<serde_json::Value>>(&contents) {
            if let Some(first) = models.first() {
                return first.clone();
            }
        }
    }
    // Fallback: synthesize from the first `<name>/runtime/<name>.model3.json`.
    if let Ok(entries) = fs::read_dir(live2d_models_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let candidate = entry
                .path()
                .join("runtime")
                .join(format!("{name}.model3.json"));
            if candidate.is_file() {
                return serde_json::json!({
                    "name": name,
                    "description": "",
                    "url": format!("/live2d-models/{name}/runtime/{name}.model3.json"),
                    "kScale": 0.5,
                    "initialXshift": 0,
                    "initialYshift": 0,
                    "kXOffset": 0,
                    "idleMotionGroupName": "Idle",
                    "emotionMap": { "neutral": 0 },
                });
            }
        }
    }
    serde_json::json!(null)
}

fn load_default_character(legacy_config_file: &Path) -> (String, String) {
    if let Ok(contents) = fs::read_to_string(legacy_config_file) {
        if let Ok(value) = serde_yaml::from_str::<serde_json::Value>(&contents) {
            if let Some(character) = value.get("character_config") {
                let name = character
                    .get("conf_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("mao_pro");
                let uid = character
                    .get("conf_uid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("mao_pro_001");
                return (name.to_owned(), uid.to_owned());
            }
        }
    }
    ("mao_pro".to_owned(), "mao_pro_001".to_owned())
}

fn yaml_file_names(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "yaml" || extension == "yml")
            {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    names
}

fn image_file_names(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|extension| {
                    matches!(
                        extension.to_string_lossy().as_ref(),
                        "jpg" | "jpeg" | "png" | "gif"
                    )
                })
            {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    names
}

#[derive(Clone)]
struct AppState {
    python_ws_url: Url,
    python_http_url: Url,
    provider_rpc: bool,
    http_client: reqwest::Client,
    connections: Arc<Semaphore>,
    peer_limiter: Arc<PeerLimiter>,
    max_message_bytes: usize,
    connect_timeout: Duration,
    http_upload_idle_timeout: Duration,
    http_response_idle_timeout: Duration,
    session_queue_capacity: usize,
    max_audio_samples: usize,
    energy_vad: EnergyVadConfig,
    allowed_origins: Arc<Vec<HeaderValue>>,
    frontend_index: Arc<String>,
    frontend_available: bool,
    settings: Arc<settings::SettingsRepository>,
    legacy_settings: Arc<legacy_settings::LegacySettingsAdapter>,
    session: Arc<tokio::sync::Mutex<session::SessionSupervisor>>,
    chat: Option<Arc<ChatRuntime>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    auth_token: Option<Arc<str>>,
    http_rate_limiter: Arc<HttpRateLimiter>,
    http_concurrency: Option<Arc<Semaphore>>,
    allow_missing_python: bool,
    init_data: Option<Arc<InitRuntimeData>>,
    metrics: Arc<Metrics>,
}

#[derive(Serialize)]
struct HealthResponse<'a> {
    status: &'a str,
    service: &'a str,
}

#[derive(Serialize)]
struct GatewayCapabilities<'a> {
    service: &'a str,
    audio_protocol_version: u8,
    audio_modes: [&'a str; 2],
    audio_encodings: [&'a str; 1],
    audio_sample_rates: [u32; 3],
    audio_channels: [u8; 1],
    max_connections_per_ip: usize,
    max_connection_attempts_per_minute: u32,
    http_upload_idle_timeout_ms: u64,
    http_response_idle_timeout_ms: u64,
    frontend_served_by_gateway: bool,
    settings_schema_version: u16,
}

#[derive(Default)]
struct Metrics {
    active_connections: AtomicU64,
    connections_total: AtomicU64,
    origin_rejections_total: AtomicU64,
    capacity_rejections_total: AtomicU64,
    peer_limit_rejections_total: AtomicU64,
    websocket_errors_total: AtomicU64,
    http_requests_total: AtomicU64,
    http_proxy_errors_total: AtomicU64,
    http_upload_timeouts_total: AtomicU64,
    http_response_timeouts_total: AtomicU64,
    client_messages_total: AtomicU64,
    client_bytes_total: AtomicU64,
    python_messages_total: AtomicU64,
    python_bytes_total: AtomicU64,
    runtime_messages_total: AtomicU64,
    runtime_bytes_total: AtomicU64,
    vad_activations_total: AtomicU64,
    normalized_audio_samples_total: AtomicU64,
    audio_segments_total: AtomicU64,
    audio_samples_total: AtomicU64,
}

impl Metrics {
    fn render(&self) -> String {
        let values = [
            (
                "olv_gateway_active_connections",
                self.active_connections.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_connections_total",
                self.connections_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_origin_rejections_total",
                self.origin_rejections_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_capacity_rejections_total",
                self.capacity_rejections_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_peer_limit_rejections_total",
                self.peer_limit_rejections_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_websocket_errors_total",
                self.websocket_errors_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_http_requests_total",
                self.http_requests_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_http_proxy_errors_total",
                self.http_proxy_errors_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_http_upload_timeouts_total",
                self.http_upload_timeouts_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_http_response_timeouts_total",
                self.http_response_timeouts_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_client_messages_total",
                self.client_messages_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_client_bytes_total",
                self.client_bytes_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_python_messages_total",
                self.python_messages_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_python_bytes_total",
                self.python_bytes_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_runtime_messages_total",
                self.runtime_messages_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_runtime_bytes_total",
                self.runtime_bytes_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_vad_activations_total",
                self.vad_activations_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_normalized_audio_samples_total",
                self.normalized_audio_samples_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_audio_segments_total",
                self.audio_segments_total.load(Ordering::Relaxed),
            ),
            (
                "olv_gateway_audio_samples_total",
                self.audio_samples_total.load(Ordering::Relaxed),
            ),
        ];
        let mut output = String::new();
        for (name, value) in values {
            output.push_str(name);
            output.push(' ');
            output.push_str(&value.to_string());
            output.push('\n');
        }
        output
    }
}

const PEER_RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_TRACKED_PEERS: usize = 4096;
const PEER_CLEANUP_INTERVAL: u64 = 256;

struct PeerLimiter {
    state: Mutex<PeerLimiterState>,
    max_connections: usize,
    max_attempts: u32,
}

#[derive(Default)]
struct PeerLimiterState {
    peers: HashMap<IpAddr, PeerState>,
    operations: u64,
}

struct PeerState {
    active_connections: usize,
    attempts: u32,
    window_started: Instant,
}

#[derive(Debug, PartialEq, Eq)]
enum PeerLimitRejection {
    ConcurrentConnections,
    ConnectionRate,
    TrackingCapacity,
}

impl PeerLimiter {
    fn new(max_connections: usize, max_attempts: u32) -> Self {
        Self {
            state: Mutex::new(PeerLimiterState::default()),
            max_connections,
            max_attempts,
        }
    }

    fn try_acquire(
        self: &Arc<Self>,
        peer: IpAddr,
    ) -> std::result::Result<PeerConnectionGuard, PeerLimitRejection> {
        let now = Instant::now();
        let mut limiter = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        limiter.operations = limiter.operations.wrapping_add(1);
        if limiter.operations % PEER_CLEANUP_INTERVAL == 0 {
            limiter.peers.retain(|_, state| {
                state.active_connections > 0
                    || now.duration_since(state.window_started) < PEER_RATE_WINDOW
            });
        }
        if !limiter.peers.contains_key(&peer) && limiter.peers.len() >= MAX_TRACKED_PEERS {
            return Err(PeerLimitRejection::TrackingCapacity);
        }

        let state = limiter.peers.entry(peer).or_insert(PeerState {
            active_connections: 0,
            attempts: 0,
            window_started: now,
        });
        if now.duration_since(state.window_started) >= PEER_RATE_WINDOW {
            state.attempts = 0;
            state.window_started = now;
        }
        if state.attempts >= self.max_attempts {
            return Err(PeerLimitRejection::ConnectionRate);
        }
        state.attempts += 1;
        if state.active_connections >= self.max_connections {
            return Err(PeerLimitRejection::ConcurrentConnections);
        }
        state.active_connections += 1;
        Ok(PeerConnectionGuard {
            limiter: self.clone(),
            peer,
        })
    }
}

struct PeerConnectionGuard {
    limiter: Arc<PeerLimiter>,
    peer: IpAddr,
}

impl Drop for PeerConnectionGuard {
    fn drop(&mut self) {
        let mut limiter = self
            .limiter
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(state) = limiter.peers.get_mut(&self.peer) {
            state.active_connections = state.active_connections.saturating_sub(1);
        }
    }
}

struct ActiveConnectionGuard(Arc<Metrics>);

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.0.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "open_llm_vtuber_gateway=info,tower_http=info".into()),
        )
        .init();

    let config = Config::parse();
    config.validate()?;
    if let Some(path) = &config.export_settings_types {
        settings::export_typescript_bindings(path)?;
        info!(path = %path.display(), "settings TypeScript bindings exported");
        return Ok(());
    }
    let (shutdown_tx, shutdown_rx) = shutdown_channel();
    let app = build_router_with_mcp(
        &config,
        build_mcp_registry(&config).await?,
        shutdown_tx,
        std::env::var("OLV_GATEWAY_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty()),
    )?;
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("failed to bind gateway to {}", config.listen))?;

    info!(listen = %config.listen, upstream = %config.python_ws_url, "gateway started");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_rx))
    .await
    .context("gateway server stopped unexpectedly")
}

/// Creates a `(sender, receiver)` pair used to trigger graceful shutdown via
/// `POST /shutdown` (in addition to Ctrl+C/SIGTERM).
fn shutdown_channel() -> (
    tokio::sync::watch::Sender<bool>,
    tokio::sync::watch::Receiver<bool>,
) {
    tokio::sync::watch::channel(false)
}

/// Router without MCP servers (used by tests). The shutdown sender is
/// discarded; graceful shutdown is exercised through the endpoint test.
#[cfg(test)]
fn build_router(config: &Config) -> Result<Router> {
    let (shutdown_tx, _shutdown_rx) = shutdown_channel();
    build_router_with_mcp(config, None, shutdown_tx, None)
}

fn build_router_with_mcp(
    config: &Config,
    mcp: Option<Arc<tokio::sync::Mutex<mcp::McpRegistry>>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    auth_token: Option<String>,
) -> Result<Router> {
    let (loaded_frontend_index, frontend_available) = load_frontend_index(&config.frontend_dir);
    let settings =
        settings::SettingsRepository::load(&config.settings_file).with_context(|| {
            format!(
                "failed to initialize settings repository at {}",
                config.settings_file.display()
            )
        })?;
    let legacy_settings = Arc::new(legacy_settings::LegacySettingsAdapter::new(
        config.legacy_config_file.clone(),
        config.legacy_characters_dir.clone(),
    ));
    let settings_arc = Arc::new(settings);
    let state = AppState {
        python_ws_url: config.python_ws_url.clone(),
        python_http_url: python_http_url(&config.python_ws_url),
        provider_rpc: is_provider_rpc_url(&config.python_ws_url),
        http_client: reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build HTTP proxy client"),
        connections: Arc::new(Semaphore::new(config.max_connections)),
        peer_limiter: Arc::new(PeerLimiter::new(
            config.max_connections_per_ip,
            config.max_connection_attempts_per_minute,
        )),
        max_message_bytes: config.max_message_bytes,
        connect_timeout: Duration::from_millis(config.connect_timeout_ms),
        http_upload_idle_timeout: Duration::from_millis(config.http_upload_idle_timeout_ms),
        http_response_idle_timeout: Duration::from_millis(config.http_response_idle_timeout_ms),
        session_queue_capacity: config.session_queue_capacity.max(1),
        max_audio_samples: config.max_audio_seconds.saturating_mul(16_000),
        energy_vad: EnergyVadConfig {
            rms_threshold: config.vad_rms_threshold,
            frame_samples: config.vad_frame_samples.max(1),
            start_frames: config.vad_start_frames.max(1),
            end_frames: config.vad_end_frames.max(1),
            pre_roll_frames: config.vad_pre_roll_frames.max(config.vad_start_frames),
        },
        allowed_origins: Arc::new(config.allowed_origins.clone()),
        frontend_index: Arc::new(loaded_frontend_index),
        frontend_available,
        settings: settings_arc.clone(),
        legacy_settings: legacy_settings.clone(),
        session: Arc::new(tokio::sync::Mutex::new(session::SessionSupervisor::new(
            config.max_transcript_entries.max(1),
        ))),
        chat: build_chat_runtime(config, &legacy_settings, mcp, &settings_arc)?,
        shutdown: shutdown_tx,
        auth_token: auth_token.map(Arc::from),
        http_rate_limiter: Arc::new(HttpRateLimiter::new(config.http_requests_per_minute_per_ip)),
        http_concurrency: (config.max_concurrent_http > 0)
            .then(|| Arc::new(Semaphore::new(config.max_concurrent_http))),
        allow_missing_python: config.allow_missing_python,
        init_data: matches!(config.chat_mode, ChatMode::Native)
            .then(|| Arc::new(build_init_runtime_data(config))),
        metrics: Arc::new(Metrics::default()),
    };

    Ok(Router::new()
        .route("/", get(frontend_index))
        .route("/healthz", get(health))
        .route("/gateway/capabilities", get(capabilities))
        .route("/api/v1/settings/schema", get(settings_schema))
        .route("/api/v1/settings/snapshot", get(settings_snapshot))
        .route("/api/v1/settings/validate", post(validate_settings))
        .route("/api/v1/settings", patch(apply_settings))
        .route("/api/v1/settings/legacy", get(legacy_settings_snapshot))
        .route("/api/v1/session", get(session_snapshot))
        .route("/api/v1/session/reset", post(session_reset))
        .route("/shutdown", post(shutdown_gateway))
        .route("/metrics", get(metrics))
        .route("/client-ws", get(client_ws))
        .route("/asr", any(proxy_http))
        .route("/live2d-models/info", get(proxy_http))
        .route("/openapi.json", get(proxy_http))
        .route("/docs", get(proxy_http))
        .route("/redoc", get(proxy_http))
        .route_service(
            "/favicon.ico",
            ServeFile::new(config.frontend_dir.join("favicon.ico")),
        )
        .nest_service("/assets", ServeDir::new(config.frontend_dir.join("assets")))
        .nest_service("/libs", ServeDir::new(config.frontend_dir.join("libs")))
        .nest_service("/cache", ServeDir::new(&config.cache_dir))
        .nest_service("/live2d-models", ServeDir::new(&config.live2d_models_dir))
        .nest_service("/bg", ServeDir::new(&config.backgrounds_dir))
        .nest_service("/avatars", ServeDir::new(&config.avatars_dir))
        .nest_service(
            "/web-tool",
            ServeDir::new(&config.web_tool_dir).append_index_html_on_directories(true),
        )
        .fallback(any(proxy_http))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            concurrency_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(state, auth_middleware))
        .layer(RequestBodyLimitLayer::new(config.max_http_body_bytes))
        .layer(TraceLayer::new_for_http()))
}

/// Builds the native chat runtime for `--chat-mode native`; returns `None`
/// in proxy mode. API keys are read from the environment, never argv.
fn build_chat_runtime(
    config: &Config,
    legacy_settings: &Arc<legacy_settings::LegacySettingsAdapter>,
    mcp: Option<Arc<tokio::sync::Mutex<mcp::McpRegistry>>>,
    settings: &settings::SettingsRepository,
) -> Result<Option<Arc<ChatRuntime>>> {
    if !matches!(config.chat_mode, ChatMode::Native) {
        return Ok(None);
    }
    // Provider configuration priority: settings domain (single source) when
    // configured, otherwise CLI/env fallback.
    let stored = settings.snapshot().provider;
    let (kind, base_url, model, api_key) = if stored.kind != settings::ProviderKindSetting::None {
        let api_key = settings.secret_plaintext("provider.api_key");
        (
            provider_kind_from_setting(stored.kind),
            stored.base_url,
            stored.model,
            api_key,
        )
    } else {
        let api_key = match config.chat_provider {
            provider::ProviderKind::OpenAi => provider_api_key("OLV_PROVIDER_OPENAI_API_KEY"),
            provider::ProviderKind::Anthropic => provider_api_key("OLV_PROVIDER_ANTHROPIC_API_KEY"),
            provider::ProviderKind::Ollama => None,
        };
        (
            config.chat_provider,
            config.chat_base_url.clone(),
            config.chat_model.clone(),
            api_key,
        )
    };
    let base_url = base_url.unwrap_or_else(|| {
        config
            .chat_base_url
            .clone()
            .unwrap_or_else(|| default_chat_base_url(kind))
    });
    let provider_config = provider::ProviderConfig {
        kind,
        base_url,
        api_key,
        default_model: model,
        timeout: Duration::from_millis(config.chat_provider_timeout_ms),
    };
    let provider = provider::build_provider(&provider_config)
        .with_context(|| "failed to initialize native chat provider")?;
    Ok(Some(Arc::new(ChatRuntime {
        provider: Arc::from(provider),
        legacy_settings: legacy_settings.clone(),
        history_limit: config.chat_history_messages,
        system_prompt: config.chat_system_prompt.clone(),
        mcp,
        max_tool_rounds: config.max_tool_rounds,
    })))
}

/// Maps a settings provider kind to the runtime provider kind. `None` maps
/// to OpenAI (the settings branch only runs when a concrete kind is stored).
fn provider_kind_from_setting(kind: settings::ProviderKindSetting) -> provider::ProviderKind {
    match kind {
        settings::ProviderKindSetting::OpenAi => provider::ProviderKind::OpenAi,
        settings::ProviderKindSetting::Anthropic => provider::ProviderKind::Anthropic,
        settings::ProviderKindSetting::Ollama => provider::ProviderKind::Ollama,
        settings::ProviderKindSetting::None => provider::ProviderKind::OpenAi,
    }
}

/// Parses `name=command args...` specs, spawns the MCP server processes and
/// performs the initialize handshake. Returns `None` when no servers are
/// configured.
async fn build_mcp_registry(
    config: &Config,
) -> Result<Option<Arc<tokio::sync::Mutex<mcp::McpRegistry>>>> {
    if config.mcp_servers.is_empty() {
        return Ok(None);
    }
    let request_timeout = Duration::from_millis(config.mcp_timeout_ms);
    let connect_timeout = Duration::from_secs(10);
    let mut registry = mcp::McpRegistry::new();
    for spec in &config.mcp_servers {
        let (name, command_line) = spec.split_once('=').with_context(|| {
            format!("MCP server spec must be `name=command args`, got '{spec}'")
        })?;
        let mut parts = command_line.split_whitespace();
        let command = parts
            .next()
            .with_context(|| format!("MCP server command is empty in '{spec}'"))?;
        let args: Vec<String> = parts.map(str::to_owned).collect();
        info!(server = %name, command, "connecting MCP server");
        let server = mcp::spawn_and_connect(
            name,
            command,
            &args,
            request_timeout,
            connect_timeout,
            &cancellation::CancellationToken::new(),
        )
        .await
        .with_context(|| format!("failed to connect MCP server '{name}'"))?;
        info!(
            server = %name,
            tools = server.tools().len(),
            "MCP server connected"
        );
        registry.insert(name.to_owned(), server);
    }
    Ok(Some(Arc::new(tokio::sync::Mutex::new(registry))))
}

fn provider_api_key(env_name: &str) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn default_chat_base_url(kind: provider::ProviderKind) -> String {
    match kind {
        provider::ProviderKind::OpenAi => "https://api.openai.com/v1".to_owned(),
        provider::ProviderKind::Anthropic => "https://api.anthropic.com".to_owned(),
        provider::ProviderKind::Ollama => "http://127.0.0.1:11434".to_owned(),
    }
}

const SAME_ORIGIN_BOOTSTRAP: &str = r#"<script>
(() => {
  const migrate = (key, legacyValues, nextValue) => {
    let current = null;
    try {
      current = JSON.parse(localStorage.getItem(key) || "null");
    } catch (_) {
      current = null;
    }
    if (!current || legacyValues.includes(current)) {
      localStorage.setItem(key, JSON.stringify(nextValue));
    }
  };
  const websocketProtocol = location.protocol === "https:" ? "wss:" : "ws:";
  migrate("wsUrl", ["ws://127.0.0.1:12393/client-ws", "ws://localhost:12393/client-ws"], `${websocketProtocol}//${location.host}/client-ws`);
  migrate("baseUrl", ["http://127.0.0.1:12393", "http://localhost:12393"], location.origin);
})();
</script>"#;

fn load_frontend_index(frontend_dir: &Path) -> (String, bool) {
    let path = frontend_dir.join("index.html");
    match fs::read_to_string(&path) {
        Ok(index) => (
            index.replacen("</head>", &format!("{SAME_ORIGIN_BOOTSTRAP}\n</head>"), 1),
            true,
        ),
        Err(error) => {
            warn!(path = %path.display(), %error, "failed to load frontend index");
            (
                "<!doctype html><title>Open-LLM-VTuber</title><h1>Frontend unavailable</h1>"
                    .to_owned(),
                false,
            )
        }
    }
}

fn is_provider_rpc_url(websocket_url: &Url) -> bool {
    websocket_url.path() == "/internal/v1/session-ws"
}

fn python_http_url(websocket_url: &Url) -> Url {
    let mut url = websocket_url.clone();
    url.set_scheme(if websocket_url.scheme() == "wss" {
        "https"
    } else {
        "http"
    })
    .expect("HTTP scheme must be valid");
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    url
}

#[derive(Clone, Copy)]
enum HttpBodyDirection {
    Upload,
    Response,
}

type ProxyBodyError = Box<dyn Error + Send + Sync>;

fn with_idle_timeout<S, T, E>(
    source: S,
    timeout: Duration,
    metrics: Arc<Metrics>,
    timeout_flag: Arc<AtomicBool>,
    direction: HttpBodyDirection,
) -> impl Stream<Item = std::result::Result<T, ProxyBodyError>> + Send
where
    S: Stream<Item = std::result::Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Error + Send + Sync + 'static,
{
    stream::unfold(
        (Box::pin(source), false, metrics),
        move |(mut source, finished, metrics)| {
            let timeout_flag = timeout_flag.clone();
            async move {
                if finished {
                    return None;
                }
                match tokio::time::timeout(timeout, source.next()).await {
                    Ok(Some(Ok(item))) => Some((Ok(item), (source, false, metrics))),
                    Ok(Some(Err(error))) => {
                        let error: ProxyBodyError = Box::new(error);
                        Some((Err(error), (source, true, metrics)))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        timeout_flag.store(true, Ordering::Relaxed);
                        match direction {
                            HttpBodyDirection::Upload => metrics
                                .http_upload_timeouts_total
                                .fetch_add(1, Ordering::Relaxed),
                            HttpBodyDirection::Response => {
                                metrics
                                    .http_response_timeouts_total
                                    .fetch_add(1, Ordering::Relaxed);
                                metrics
                                    .http_proxy_errors_total
                                    .fetch_add(1, Ordering::Relaxed)
                            }
                        };
                        let message = match direction {
                            HttpBodyDirection::Upload => "HTTP upload body idle timeout",
                            HttpBodyDirection::Response => "HTTP response body idle timeout",
                        };
                        let error: ProxyBodyError =
                            Box::new(io::Error::new(io::ErrorKind::TimedOut, message));
                        Some((Err(error), (source, true, metrics)))
                    }
                }
            }
        },
    )
}

fn error_is_timeout(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if candidate
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::TimedOut)
        {
            return true;
        }
        current = candidate.source();
    }
    false
}

async fn proxy_http(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, StatusCode> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let (parts, body) = request.into_parts();
    let mut upstream_url = state.python_http_url.clone();
    upstream_url.set_path(parts.uri.path());
    upstream_url.set_query(parts.uri.query());

    let upload_timed_out = Arc::new(AtomicBool::new(false));
    let upload_stream = with_idle_timeout(
        body.into_data_stream(),
        state.http_upload_idle_timeout,
        state.metrics.clone(),
        upload_timed_out.clone(),
        HttpBodyDirection::Upload,
    );
    let mut upstream_request = state
        .http_client
        .request(parts.method, upstream_url)
        .body(reqwest::Body::wrap_stream(upload_stream));
    for (name, value) in &parts.headers {
        if !is_hop_by_hop_header(name.as_str()) && name.as_str() != "host" {
            upstream_request = upstream_request.header(name, value);
        }
    }

    let upstream_result = upstream_request.send().await;
    if upload_timed_out.load(Ordering::Relaxed) {
        state
            .metrics
            .http_proxy_errors_total
            .fetch_add(1, Ordering::Relaxed);
        warn!("HTTP upload timed out while proxying to Python runtime");
        return Err(StatusCode::REQUEST_TIMEOUT);
    }
    let upstream_response = upstream_result.map_err(|error| {
        state
            .metrics
            .http_proxy_errors_total
            .fetch_add(1, Ordering::Relaxed);
        if error_is_timeout(&error) {
            warn!(%error, "HTTP upload timed out while proxying to Python runtime");
            StatusCode::REQUEST_TIMEOUT
        } else {
            warn!(%error, "failed to proxy HTTP request to Python runtime");
            StatusCode::BAD_GATEWAY
        }
    })?;
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let response_timeout = Arc::new(AtomicBool::new(false));
    let response_stream = with_idle_timeout(
        upstream_response.bytes_stream(),
        state.http_response_idle_timeout,
        state.metrics.clone(),
        response_timeout,
        HttpBodyDirection::Response,
    );
    let mut response = Response::new(Body::from_stream(response_stream));
    *response.status_mut() = status;
    for (name, value) in &headers {
        if !is_hop_by_hop_header(name.as_str()) {
            response.headers_mut().append(name, value.clone());
        }
    }
    Ok(response)
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn frontend_index(State(state): State<AppState>) -> impl IntoResponse {
    (
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        state.frontend_index.as_str().to_owned(),
    )
}

async fn health() -> impl IntoResponse {
    axum::Json(HealthResponse {
        status: "ok",
        service: "open-llm-vtuber-gateway",
    })
}

async fn capabilities(State(state): State<AppState>) -> impl IntoResponse {
    axum::Json(GatewayCapabilities {
        service: "open-llm-vtuber-gateway",
        audio_protocol_version: audio_protocol_version(),
        audio_modes: ["manual", "energy-vad"],
        audio_encodings: ["pcm_s16le"],
        audio_sample_rates: SUPPORTED_AUDIO_SAMPLE_RATES,
        audio_channels: [1],
        max_connections_per_ip: state.peer_limiter.max_connections,
        max_connection_attempts_per_minute: state.peer_limiter.max_attempts,
        http_upload_idle_timeout_ms: state.http_upload_idle_timeout.as_millis() as u64,
        http_response_idle_timeout_ms: state.http_response_idle_timeout.as_millis() as u64,
        frontend_served_by_gateway: state.frontend_available,
        settings_schema_version: settings::SETTINGS_SCHEMA_VERSION,
    })
}

async fn settings_schema() -> impl IntoResponse {
    axum::Json(settings::schema_response())
}

async fn settings_snapshot(State(state): State<AppState>) -> Response {
    no_store(axum::Json(state.settings.snapshot()).into_response())
}

async fn legacy_settings_snapshot(State(state): State<AppState>) -> Response {
    no_store(axum::Json(state.legacy_settings.snapshot()).into_response())
}

async fn session_snapshot(State(state): State<AppState>) -> Response {
    let snapshot = state.session.lock().await.snapshot();
    no_store(axum::Json(snapshot).into_response())
}

async fn session_reset(State(state): State<AppState>) -> Response {
    state.session.lock().await.reset();
    no_store(axum::Json(serde_json::json!({ "ok": true })).into_response())
}

/// Triggers graceful shutdown. Used by the desktop supervisor on every
/// platform (Windows has no SIGTERM semantics); Ctrl+C/SIGTERM remain.
async fn shutdown_gateway(State(state): State<AppState>) -> Response {
    state.shutdown.send(true).ok();
    no_store(axum::Json(serde_json::json!({ "status": "shutting_down" })).into_response())
}

async fn validate_settings(
    axum::Json(snapshot): axum::Json<settings::SettingsSnapshotV1>,
) -> impl IntoResponse {
    axum::Json(settings::validation_response(&snapshot))
}

async fn apply_settings(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<settings::SettingsPatchRequestV1>,
) -> Response {
    let repository = state.settings.clone();
    match tokio::task::spawn_blocking(move || repository.apply(request)).await {
        Ok(Ok(applied)) => no_store(axum::Json(applied).into_response()),
        Ok(Err(settings::SettingsApplyError::Conflict(snapshot))) => no_store(
            (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "error": {
                        "code": "revision_conflict",
                        "message": "settings changed since the draft was created"
                    },
                    "snapshot": snapshot
                })),
            )
                .into_response(),
        ),
        Ok(Err(settings::SettingsApplyError::Validation(errors))) => no_store(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": {
                        "code": "validation_failed",
                        "message": "settings validation failed"
                    },
                    "errors": errors
                })),
            )
                .into_response(),
        ),
        Ok(Err(settings::SettingsApplyError::RevisionExhausted)) => no_store(
            (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "error": {
                        "code": "revision_exhausted",
                        "message": "settings revision cannot be incremented"
                    }
                })),
            )
                .into_response(),
        ),
        Ok(Err(settings::SettingsApplyError::Storage(error))) => {
            warn!(error = %error, "failed to persist settings");
            no_store(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({
                        "error": {
                            "code": "storage_error",
                            "message": "settings could not be persisted"
                        }
                    })),
                )
                    .into_response(),
            )
        }
        Err(error) => {
            warn!(error = %error, "settings persistence task failed");
            no_store(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({
                        "error": {
                            "code": "internal_error",
                            "message": "settings could not be applied"
                        }
                    })),
                )
                    .into_response(),
            )
        }
    }
}

/// Cache-control + permissive CORS for the loopback management API. The
/// gateway serves the frontend same-origin in packaged builds, but the Vite
/// dev server (localhost:3000) fetches cross-origin; responses only carry
/// redacted/derived data, never plaintext secrets.
fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type,authorization"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,POST,PATCH,OPTIONS"),
    );
    response
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.render(),
    )
}

async fn client_ws(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !origin_is_allowed(&headers, &state.allowed_origins) {
        state
            .metrics
            .origin_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return StatusCode::FORBIDDEN.into_response();
    }

    let peer_permit = match state.peer_limiter.try_acquire(peer.ip()) {
        Ok(permit) => permit,
        Err(rejection) => {
            state
                .metrics
                .peer_limit_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            warn!(%peer, ?rejection, "rejected websocket connection by peer limit");
            let retry_after = match rejection {
                PeerLimitRejection::ConcurrentConnections => "1",
                PeerLimitRejection::ConnectionRate | PeerLimitRejection::TrackingCapacity => "60",
            };
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, HeaderValue::from_static(retry_after))],
            )
                .into_response();
        }
    };

    let permit = match state.connections.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            state
                .metrics
                .capacity_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    websocket
        .max_message_size(state.max_message_bytes)
        .max_frame_size(state.max_message_bytes)
        .on_upgrade(move |socket| async move {
            let connection_id = Uuid::new_v4();
            let span = info_span!("websocket", %connection_id, %peer);
            async move {
                let _permit = permit;
                let _peer_permit = peer_permit;
                state
                    .metrics
                    .connections_total
                    .fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .active_connections
                    .fetch_add(1, Ordering::Relaxed);
                let _active_connection = ActiveConnectionGuard(state.metrics.clone());
                if let Err(error) = proxy_connection(socket, &state).await {
                    state
                        .metrics
                        .websocket_errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    warn!(%error, "websocket proxy stopped with an error");
                }
            }
            .instrument(span)
            .await;
        })
}

fn origin_is_allowed(headers: &HeaderMap, allowed_origins: &[HeaderValue]) -> bool {
    headers
        .get(ORIGIN)
        .is_none_or(|origin| allowed_origins.iter().any(|allowed| allowed == origin))
}

async fn proxy_connection(client: WebSocket, state: &AppState) -> Result<()> {
    let upstream = match tokio::time::timeout(
        state.connect_timeout,
        connect_async(state.python_ws_url.as_str()),
    )
    .await
    {
        Ok(Ok((websocket, _))) => Some(websocket),
        Ok(Err(error)) if state.allow_missing_python => {
            warn!(
                error = %error,
                "Python runtime unavailable; continuing without upstream"
            );
            None
        }
        Ok(Err(error)) => {
            return Err(error).context("failed to connect to Python runtime");
        }
        Err(_) if state.allow_missing_python => {
            warn!("timed out connecting to Python runtime; continuing without upstream");
            None
        }
        Err(_) => {
            return Err(anyhow::anyhow!("timed out connecting to Python runtime"));
        }
    };

    info!(
        upstream = %state.python_ws_url,
        connected = upstream.is_some(),
        "session upstream established"
    );

    SessionActor::new(
        client,
        upstream,
        SessionActorConfig {
            queue_capacity: state.session_queue_capacity,
            max_audio_samples: state.max_audio_samples,
            energy_vad: state.energy_vad,
            provider_rpc: state.provider_rpc,
            chat: state.chat.clone(),
            init_data: state.init_data.clone(),
        },
        state.session.clone(),
        state.metrics.clone(),
    )
    .run()
    .await
}

struct SessionActorConfig {
    queue_capacity: usize,
    max_audio_samples: usize,
    energy_vad: EnergyVadConfig,
    provider_rpc: bool,
    chat: Option<Arc<ChatRuntime>>,
    init_data: Option<Arc<InitRuntimeData>>,
}

struct SessionActor {
    client: WebSocket,
    upstream: Option<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    queue_capacity: usize,
    max_audio_samples: usize,
    energy_vad: EnergyVadConfig,
    provider_rpc: bool,
    chat: Option<Arc<ChatRuntime>>,
    init_data: Option<Arc<InitRuntimeData>>,
    session: Arc<tokio::sync::Mutex<session::SessionSupervisor>>,
    metrics: Arc<Metrics>,
}

impl SessionActor {
    fn new(
        client: WebSocket,
        upstream: Option<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        config: SessionActorConfig,
        session: Arc<tokio::sync::Mutex<session::SessionSupervisor>>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            client,
            upstream,
            queue_capacity: config.queue_capacity,
            max_audio_samples: config.max_audio_samples,
            energy_vad: config.energy_vad,
            provider_rpc: config.provider_rpc,
            chat: config.chat,
            init_data: config.init_data,
            session,
            metrics,
        }
    }

    async fn run(self) -> Result<()> {
        let (mut client_tx, mut client_rx) = self.client.split();
        let (to_upstream_tx, mut to_upstream_rx) = mpsc::channel(self.queue_capacity);
        let (to_client_tx, mut to_client_rx) = mpsc::channel(self.queue_capacity);
        let mut audio_state = AudioSessionState::new(self.max_audio_samples, self.energy_vad);
        let client_metrics = self.metrics.clone();
        let python_metrics = self.metrics.clone();
        let runtime_metrics = self.metrics;
        let provider_rpc = self.provider_rpc;
        let client_session = self.session.clone();
        let upstream_session = self.session;
        let chat_runtime = self.chat;
        let init_data = self.init_data;

        // Native mode: answer the frontend's initialization protocol directly
        // (the Python runtime is not present).
        let init_client_tx = to_client_tx.clone();
        if let Some(init) = &init_data {
            let welcome = client_text_message(&serde_json::json!({
                "type": "set-model-and-conf",
                "model_info": init.model_info,
                "conf_name": init.conf_name,
                "conf_uid": init.conf_uid,
                "client_uid": "rust-native",
            }));
            if init_client_tx.send(welcome).await.is_err() {
                return Ok(());
            }
        }

        // Native chat orchestration channel (only in `--chat-mode native`).
        let (chat_tx, chat_rx) = mpsc::channel::<OrchestratorCommand>(self.queue_capacity);
        if let Some(runtime) = &chat_runtime {
            tokio::spawn(run_orchestrator(
                chat_rx,
                to_client_tx.clone(),
                to_upstream_tx.clone(),
                provider_rpc,
                runtime.clone(),
            ));
        }
        let chat_tx = chat_runtime.as_ref().map(|_| chat_tx);
        let upstream_chat_tx = chat_tx.clone();
        let client_init_data = init_data.clone();
        let read_client_init_tx = init_client_tx;

        let read_client = async move {
            while let Some(message) = client_rx.next().await {
                let message = message.context("failed to read client message")?;
                client_metrics
                    .client_messages_total
                    .fetch_add(1, Ordering::Relaxed);
                client_metrics
                    .client_bytes_total
                    .fetch_add(axum_message_size(&message), Ordering::Relaxed);
                if let axum::extract::ws::Message::Text(text) = &message {
                    if let Some(signal) = session::parse_client_signal(text.as_str()) {
                        client_session.lock().await.observe_client_signal(&signal);
                        // Native mode: serve the initialization protocol locally.
                        if let Some(init) = &client_init_data {
                            let response = match signal.message_type.as_str() {
                                "fetch-configs" => Some(serde_json::json!({
                                    "type": "config-files",
                                    "configs": init.config_files,
                                })),
                                "fetch-backgrounds" => Some(serde_json::json!({
                                    "type": "background-files",
                                    "files": init.background_files,
                                })),
                                "request-init-config" => Some(serde_json::json!({
                                    "type": "set-model-and-conf",
                                    "model_info": init.model_info,
                                    "conf_name": init.conf_name,
                                    "conf_uid": init.conf_uid,
                                    "client_uid": "rust-native",
                                })),
                                _ => None,
                            };
                            if let Some(response) = response {
                                read_client_init_tx
                                    .send(client_text_message(&response))
                                    .await?;
                                continue;
                            }
                        }
                        if let Some(chat_tx) = &chat_tx {
                            match signal.message_type.as_str() {
                                "text-input" | "ai-speak-signal" => {
                                    let input = signal.text.unwrap_or_default();
                                    if !input.trim().is_empty() {
                                        chat_tx
                                            .send(OrchestratorCommand::Input { text: input })
                                            .await?;
                                    }
                                    continue;
                                }
                                "interrupt-signal" => {
                                    chat_tx.send(OrchestratorCommand::Interrupt).await?;
                                    continue;
                                }
                                "switch-config" => {
                                    if let Some(file) = signal.file {
                                        chat_tx
                                            .send(OrchestratorCommand::SwitchCharacter { file })
                                            .await?;
                                    }
                                    // Still forwarded: the Python runtime manages
                                    // the character list and config-switched ack.
                                }
                                _ => {}
                            }
                        }
                    }
                }
                let input_is_binary = matches!(message, axum::extract::ws::Message::Binary(_));
                let close = matches!(message, axum::extract::ws::Message::Close(_));
                let normalized_before = audio_state.normalized_samples_total();
                let messages = client_message_to_upstream(message, &mut audio_state)?;
                client_metrics.normalized_audio_samples_total.fetch_add(
                    audio_state.normalized_samples_total() - normalized_before,
                    Ordering::Relaxed,
                );
                for message in messages {
                    record_audio_output(&client_metrics, &message, input_is_binary);
                    let message = if provider_rpc {
                        wrap_provider_message(message)?
                    } else {
                        message
                    };
                    if chat_tx.is_some() {
                        let is_mic_audio_end = matches!(
                            &message,
                            tungstenite::Message::Text(text)
                                if text.as_str() == r#"{"type":"mic-audio-end"}"#
                        );
                        if is_mic_audio_end {
                            // Native mode: the buffered PCM16 audio is already
                            // forwarded to Python; ask it to transcribe instead
                            // of starting a full Python conversation.
                            let request = wrap_for_upstream(
                                tungstenite::Message::Text(r#"{"type":"asr-transcribe"}"#.into()),
                                provider_rpc,
                            )?;
                            to_upstream_tx.send(request).await?;
                            continue;
                        }
                    }
                    to_upstream_tx.send(message).await?;
                }
                if close {
                    break;
                }
            }
            Result::<()>::Ok(())
        };

        // Bridges the client reader to the Python runtime. When no upstream is
        // available (`--allow-missing-python`), forwarded messages are drained
        // and the worker never produces client output on its own.
        let upstream_worker = async move {
            match self.upstream {
                Some(upstream) => {
                    let (mut upstream_tx, mut upstream_rx) = upstream.split();
                    let write_upstream = async move {
                        while let Some(message) = to_upstream_rx.recv().await {
                            python_metrics
                                .python_messages_total
                                .fetch_add(1, Ordering::Relaxed);
                            python_metrics
                                .python_bytes_total
                                .fetch_add(tungstenite_message_size(&message), Ordering::Relaxed);
                            upstream_tx.send(message).await?;
                        }
                        Result::<()>::Ok(())
                    };
                    let read_upstream = async move {
                        while let Some(message) = upstream_rx.next().await {
                            let message =
                                message.context("failed to read Python runtime message")?;
                            runtime_metrics
                                .runtime_messages_total
                                .fetch_add(1, Ordering::Relaxed);
                            runtime_metrics
                                .runtime_bytes_total
                                .fetch_add(tungstenite_message_size(&message), Ordering::Relaxed);
                            if let tungstenite::Message::Text(text) = &message {
                                if let Some(signal) = session::parse_upstream_signal(text.as_str())
                                {
                                    upstream_session
                                        .lock()
                                        .await
                                        .observe_upstream_signal(&signal);
                                    if let Some(chat_tx) = &upstream_chat_tx {
                                        if signal.message_type == "asr-result" {
                                            chat_tx
                                                .send(OrchestratorCommand::AsrResult {
                                                    text: signal.text.unwrap_or_default(),
                                                })
                                                .await?;
                                            continue;
                                        }
                                    }
                                }
                            }
                            let close = matches!(message, tungstenite::Message::Close(_));
                            let message = if provider_rpc {
                                unwrap_provider_message(message)?
                            } else {
                                message
                            };
                            to_client_tx.send(to_client(message)).await?;
                            if close {
                                break;
                            }
                        }
                        Result::<()>::Ok(())
                    };
                    tokio::select! {
                        result = write_upstream => result?,
                        result = read_upstream => result?,
                    }
                    Result::<()>::Ok(())
                }
                None => {
                    while let Some(_message) = to_upstream_rx.recv().await {
                        // No Python runtime: drop forwarded messages.
                    }
                    Result::<()>::Ok(())
                }
            }
        };

        let write_client = async move {
            while let Some(message) = to_client_rx.recv().await {
                client_tx.send(message).await?;
            }
            Result::<()>::Ok(())
        };

        tokio::select! {
            result = read_client => result?,
            result = upstream_worker => result?,
            result = write_client => result?,
        }

        info!("websocket session actor closed");
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct EnergyVadConfig {
    rms_threshold: f32,
    frame_samples: usize,
    start_frames: usize,
    end_frames: usize,
    pre_roll_frames: usize,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum AudioMode {
    #[default]
    Manual,
    EnergyVad,
}

#[derive(Deserialize)]
struct AudioStart {
    #[serde(default = "audio_protocol_version")]
    version: u8,
    encoding: String,
    sample_rate: u32,
    channels: u8,
    #[serde(default)]
    mode: AudioMode,
}

#[derive(Deserialize)]
struct AudioEnd {
    #[serde(default)]
    images: Option<serde_json::Value>,
}

const TARGET_AUDIO_SAMPLE_RATE: u32 = 16_000;
const SUPPORTED_AUDIO_SAMPLE_RATES: [u32; 3] = [TARGET_AUDIO_SAMPLE_RATE, 44_100, 48_000];

const fn audio_protocol_version() -> u8 {
    1
}

struct Pcm16Normalizer {
    source_sample_rate: u32,
    pending_byte: Option<u8>,
    weighted_sum: i64,
    output_weight: u32,
}

impl Pcm16Normalizer {
    fn new(source_sample_rate: u32) -> Result<Self> {
        if !SUPPORTED_AUDIO_SAMPLE_RATES.contains(&source_sample_rate) {
            bail!("unsupported PCM sample rate: {source_sample_rate}");
        }
        Ok(Self {
            source_sample_rate,
            pending_byte: None,
            weighted_sum: 0,
            output_weight: 0,
        })
    }

    fn push(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        let mut offset = 0;

        if let Some(low_byte) = self.pending_byte.take() {
            if let Some(&high_byte) = data.first() {
                self.push_sample(i16::from_le_bytes([low_byte, high_byte]), &mut output);
                offset = 1;
            } else {
                self.pending_byte = Some(low_byte);
                return output;
            }
        }

        while offset + 1 < data.len() {
            self.push_sample(
                i16::from_le_bytes([data[offset], data[offset + 1]]),
                &mut output,
            );
            offset += 2;
        }
        if offset < data.len() {
            self.pending_byte = Some(data[offset]);
        }
        output
    }

    fn finish(&mut self) -> Result<Vec<u8>> {
        if self.pending_byte.is_some() {
            bail!("PCM16-LE stream ended with an incomplete sample");
        }
        let mut output = Vec::with_capacity(2);
        if self.output_weight > 0 {
            self.emit_output(&mut output);
        }
        Ok(output)
    }

    fn push_sample(&mut self, sample: i16, output: &mut Vec<u8>) {
        let mut source_weight = TARGET_AUDIO_SAMPLE_RATE;
        while source_weight > 0 {
            let output_remaining = self.source_sample_rate - self.output_weight;
            let consumed_weight = source_weight.min(output_remaining);
            self.weighted_sum += i64::from(sample) * i64::from(consumed_weight);
            self.output_weight += consumed_weight;
            source_weight -= consumed_weight;
            if self.output_weight == self.source_sample_rate {
                self.emit_output(output);
            }
        }
    }

    fn emit_output(&mut self, output: &mut Vec<u8>) {
        let divisor = i64::from(self.output_weight);
        let rounded = if self.weighted_sum >= 0 {
            (self.weighted_sum + divisor / 2) / divisor
        } else {
            (self.weighted_sum - divisor / 2) / divisor
        };
        output.extend_from_slice(&(rounded as i16).to_le_bytes());
        self.weighted_sum = 0;
        self.output_weight = 0;
    }
}

struct AudioSessionState {
    mode: Option<AudioMode>,
    pcm_bytes: Vec<u8>,
    max_samples: usize,
    energy_vad: EnergyVadState,
    normalizer: Option<Pcm16Normalizer>,
    normalized_samples_total: u64,
}

impl AudioSessionState {
    fn new(max_samples: usize, energy_vad_config: EnergyVadConfig) -> Self {
        Self {
            mode: None,
            pcm_bytes: Vec::new(),
            max_samples: max_samples.max(1),
            energy_vad: EnergyVadState::new(energy_vad_config),
            normalizer: None,
            normalized_samples_total: 0,
        }
    }

    fn start(&mut self, start: &AudioStart) -> Result<()> {
        if start.version != audio_protocol_version() {
            bail!("unsupported audio protocol version: {}", start.version);
        }
        if start.encoding != "pcm_s16le"
            || !SUPPORTED_AUDIO_SAMPLE_RATES.contains(&start.sample_rate)
            || start.channels != 1
        {
            bail!("audio-start must declare mono pcm_s16le at 16000, 44100, or 48000 Hz");
        }
        if self.mode.is_some() {
            bail!("audio-start received while an audio session is active");
        }
        self.mode = Some(start.mode);
        self.pcm_bytes.clear();
        self.energy_vad.reset();
        self.normalizer = Some(Pcm16Normalizer::new(start.sample_rate)?);
        Ok(())
    }

    fn append_pcm16le(&mut self, data: &[u8]) -> Result<Vec<tungstenite::Message>> {
        if self.mode.is_none() {
            bail!("binary audio requires an audio-start message");
        }
        let normalized = self
            .normalizer
            .as_mut()
            .context("active audio session is missing its PCM normalizer")?
            .push(data);
        self.append_normalized_pcm16le(&normalized)
    }

    fn append_normalized_pcm16le(&mut self, data: &[u8]) -> Result<Vec<tungstenite::Message>> {
        let messages = match self.mode {
            Some(AudioMode::Manual) => {
                let sample_count = data.len() / 2;
                if self.pcm_bytes.len() / 2 + sample_count > self.max_samples {
                    bail!("audio segment exceeds configured duration limit");
                }
                self.pcm_bytes.extend_from_slice(data);
                Ok(Vec::new())
            }
            Some(AudioMode::EnergyVad) => self.energy_vad.push(data, self.max_samples),
            None => bail!("binary audio requires an audio-start message"),
        }?;
        self.normalized_samples_total += (data.len() / 2) as u64;
        Ok(messages)
    }

    fn normalized_samples_total(&self) -> u64 {
        self.normalized_samples_total
    }

    fn end(&mut self) -> Result<Vec<tungstenite::Message>> {
        let mode = self
            .mode
            .context("audio-end received without an active audio segment")?;
        let tail = self
            .normalizer
            .as_mut()
            .context("active audio session is missing its PCM normalizer")?
            .finish()?;
        let mut messages = self.append_normalized_pcm16le(&tail)?;
        self.mode = None;
        self.normalizer = None;
        match mode {
            AudioMode::Manual => {
                messages.extend(audio_segment_messages(std::mem::take(&mut self.pcm_bytes)));
            }
            AudioMode::EnergyVad => messages.extend(self.energy_vad.finish()),
        }
        Ok(messages)
    }
}

struct EnergyVadState {
    config: EnergyVadConfig,
    pending_bytes: Vec<u8>,
    pre_roll: VecDeque<Vec<u8>>,
    speech_bytes: Vec<u8>,
    active: bool,
    hit_frames: usize,
    silent_frames: usize,
}

impl EnergyVadState {
    fn new(config: EnergyVadConfig) -> Self {
        Self {
            config,
            pending_bytes: Vec::new(),
            pre_roll: VecDeque::with_capacity(config.pre_roll_frames),
            speech_bytes: Vec::new(),
            active: false,
            hit_frames: 0,
            silent_frames: 0,
        }
    }

    fn reset(&mut self) {
        self.pending_bytes.clear();
        self.pre_roll.clear();
        self.speech_bytes.clear();
        self.active = false;
        self.hit_frames = 0;
        self.silent_frames = 0;
    }

    fn push(&mut self, data: &[u8], max_samples: usize) -> Result<Vec<tungstenite::Message>> {
        self.pending_bytes.extend_from_slice(data);
        let frame_bytes = self.config.frame_samples * 2;
        let complete_bytes = self.pending_bytes.len() / frame_bytes * frame_bytes;
        let mut messages = Vec::new();

        for offset in (0..complete_bytes).step_by(frame_bytes) {
            let frame = self.pending_bytes[offset..offset + frame_bytes].to_vec();
            messages.extend(self.process_frame(frame, max_samples));
        }
        self.pending_bytes.drain(..complete_bytes);
        Ok(messages)
    }

    fn process_frame(&mut self, frame: Vec<u8>, max_samples: usize) -> Vec<tungstenite::Message> {
        let voiced = pcm16le_rms(&frame) >= self.config.rms_threshold;
        if !self.active {
            self.pre_roll.push_back(frame);
            while self.pre_roll.len() > self.config.pre_roll_frames {
                self.pre_roll.pop_front();
            }
            self.hit_frames = if voiced { self.hit_frames + 1 } else { 0 };
            if self.hit_frames < self.config.start_frames {
                return Vec::new();
            }

            self.active = true;
            self.silent_frames = 0;
            self.hit_frames = 0;
            while let Some(buffered) = self.pre_roll.pop_front() {
                self.speech_bytes.extend_from_slice(&buffered);
            }
            return vec![tungstenite::Message::Text(
                r#"{"type":"interrupt-signal","text":""}"#.into(),
            )];
        }

        self.speech_bytes.extend_from_slice(&frame);
        self.silent_frames = if voiced { 0 } else { self.silent_frames + 1 };
        if self.silent_frames >= self.config.end_frames
            || self.speech_bytes.len() / 2 >= max_samples
        {
            self.active = false;
            self.silent_frames = 0;
            return audio_segment_messages(std::mem::take(&mut self.speech_bytes));
        }
        Vec::new()
    }

    fn finish(&mut self) -> Vec<tungstenite::Message> {
        self.pending_bytes.clear();
        self.pre_roll.clear();
        self.hit_frames = 0;
        self.silent_frames = 0;
        if !self.active {
            return Vec::new();
        }
        self.active = false;
        audio_segment_messages(std::mem::take(&mut self.speech_bytes))
    }
}

fn pcm16le_rms(frame: &[u8]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_squares = frame
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f64)
        .map(|sample| sample * sample)
        .sum::<f64>();
    ((sum_squares / (frame.len() / 2) as f64).sqrt() / 32768.0) as f32
}

fn audio_segment_messages(pcm_bytes: Vec<u8>) -> Vec<tungstenite::Message> {
    let mut messages = Vec::with_capacity(2);
    if !pcm_bytes.is_empty() {
        messages.push(tungstenite::Message::Binary(pcm_bytes.into()));
    }
    messages.push(tungstenite::Message::Text(
        r#"{"type":"mic-audio-end"}"#.into(),
    ));
    messages
}

const PROVIDER_RPC_VERSION: u8 = 1;
const PROVIDER_RPC_BINARY_PREFIX: &[u8] = b"OLV-RPC/1\0";

fn wrap_provider_message(message: tungstenite::Message) -> Result<tungstenite::Message> {
    match message {
        tungstenite::Message::Text(text) => Ok(tungstenite::Message::Text(
            serde_json::json!({
                "version": PROVIDER_RPC_VERSION,
                "kind": "text",
                "payload": text.as_str(),
            })
            .to_string()
            .into(),
        )),
        tungstenite::Message::Binary(data) => {
            let mut envelope = Vec::with_capacity(PROVIDER_RPC_BINARY_PREFIX.len() + data.len());
            envelope.extend_from_slice(PROVIDER_RPC_BINARY_PREFIX);
            envelope.extend_from_slice(&data);
            Ok(tungstenite::Message::Binary(envelope.into()))
        }
        other => Ok(other),
    }
}

fn unwrap_provider_message(message: tungstenite::Message) -> Result<tungstenite::Message> {
    match message {
        tungstenite::Message::Text(text) => {
            let envelope: serde_json::Value = serde_json::from_str(text.as_str())?;
            if envelope["version"] != PROVIDER_RPC_VERSION || envelope["kind"] != "text" {
                bail!("invalid provider RPC text envelope");
            }
            let payload = envelope["payload"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("provider RPC text payload must be a string"))?;
            Ok(tungstenite::Message::Text(payload.to_owned().into()))
        }
        tungstenite::Message::Binary(data) => {
            if !data.starts_with(PROVIDER_RPC_BINARY_PREFIX) {
                bail!("invalid provider RPC binary envelope");
            }
            Ok(tungstenite::Message::Binary(
                data[PROVIDER_RPC_BINARY_PREFIX.len()..].to_vec().into(),
            ))
        }
        other => Ok(other),
    }
}

/// Commands sent from the client/upstream readers to the native chat
/// orchestrator.
enum OrchestratorCommand {
    Input {
        text: String,
    },
    /// Transcribed microphone input from the Python ASR path.
    AsrResult {
        text: String,
    },
    Interrupt,
    SwitchCharacter {
        file: String,
    },
}

/// Runs the native chat session for one client connection. Provider calls
/// run as spawned tasks so that new inputs and interrupts are handled while
/// a turn is still streaming.
async fn run_orchestrator(
    mut commands: mpsc::Receiver<OrchestratorCommand>,
    client_tx: mpsc::Sender<axum::extract::ws::Message>,
    upstream_tx: mpsc::Sender<tungstenite::Message>,
    provider_rpc: bool,
    runtime: Arc<ChatRuntime>,
) {
    let mut session = conversation::ChatSession::new(
        runtime.provider.clone(),
        runtime.history_limit,
        runtime.system_prompt.clone(),
    );
    let mut running_turn: Option<tokio::task::JoinHandle<conversation::TurnOutcome>> = None;

    loop {
        let outcome = tokio::select! {
            command = commands.recv() => {
                match command {
                    None => break,
                    Some(OrchestratorCommand::Interrupt) => {
                        session.cancel_turn();
                        continue;
                    }
                    Some(OrchestratorCommand::SwitchCharacter { file }) => {
                        let prompt = conversation::character_prompt(&runtime.legacy_settings, &file);
                        session.set_character_prompt(prompt);
                        continue;
                    }
                    Some(OrchestratorCommand::Input { text })
                    | Some(OrchestratorCommand::AsrResult { text }) => {
                        if text.trim().is_empty() {
                            continue;
                        }
                        session.start_turn(text);
                        let Some(turn) = session.take_active_turn() else {
                            continue;
                        };
                        running_turn = Some(tokio::spawn(conversation::run_active_turn(
                            runtime.provider.clone(),
                            turn,
                        )));
                        continue;
                    }
                }
            }
            result = async {
                match running_turn.as_mut() {
                    Some(handle) => Some(handle.await),
                    None => std::future::pending().await,
                }
            } => {
                result
                    .expect("orchestrator turn task panicked")
                    .unwrap_or(conversation::TurnOutcome::Cancelled)
            }
        };

        running_turn = None;
        // Agent tool loop: if the provider requested tool calls and an MCP
        // registry is available, execute them, feed the results back, and
        // ask the provider again (bounded by max_tool_rounds).
        let mut outcome = outcome;
        let mut tool_rounds = 0;
        loop {
            let wants_tools = matches!(
                &outcome,
                conversation::TurnOutcome::Completed(response)
                    if !response.tool_calls.is_empty()
            );
            if !wants_tools || tool_rounds >= runtime.max_tool_rounds {
                break;
            }
            let conversation::TurnOutcome::Completed(response) = &outcome else {
                break;
            };
            let calls: Vec<provider::ToolCallRequest> = response
                .tool_calls
                .iter()
                .map(|call| provider::ToolCallRequest {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect();
            session.record_assistant_tool_calls(calls);
            let Some(registry) = &runtime.mcp else {
                // No MCP servers configured: surface a readable placeholder.
                let placeholder = response
                    .tool_calls
                    .first()
                    .map(|call| format!("(tool call requested: {})", call.name))
                    .unwrap_or_else(|| "(tool call requested)".to_owned());
                session.record_reply(placeholder.clone());
                outcome = conversation::TurnOutcome::Completed(provider::ProviderResponse {
                    text: placeholder,
                    ..Default::default()
                });
                break;
            };
            let mut tool_messages = Vec::new();
            for call in &response.tool_calls {
                let result = execute_tool(registry, &call.name, call.arguments.clone()).await;
                tool_messages.push((call.id.clone(), result));
            }
            for (call_id, result) in tool_messages {
                session.append_tool_result(call_id, result);
            }
            session.start_tool_followup();
            let Some(turn) = session.take_active_turn() else {
                break;
            };
            outcome = conversation::run_active_turn(runtime.provider.clone(), turn).await;
            tool_rounds += 1;
        }

        let payloads = match &outcome {
            conversation::TurnOutcome::Completed(response) => {
                if let Some(response) = response.tool_calls.first() {
                    // Tool calls are not executed in this milestone; report
                    // them as text so the client sees a non-empty reply.
                    let message = if response.name.is_empty() {
                        "(tool call requested)".to_owned()
                    } else {
                        format!("(tool call requested: {})", response.name)
                    };
                    session.record_reply(message.clone());
                    vec![
                        client_text_message(&serde_json::json!({
                            "type": "full-text",
                            "text": message
                        })),
                        client_text_message(
                            &serde_json::json!({ "type": "conversation-chain-end" }),
                        ),
                    ]
                } else if response.text.is_empty() {
                    vec![client_text_message(&serde_json::json!({
                        "type": "conversation-chain-end"
                    }))]
                } else {
                    session.record_reply(response.text.clone());
                    // Ask the Python sidecar to synthesize the reply (TTS only,
                    // no LLM). The audio payload streams back to the client
                    // through the normal upstream channel.
                    let tts_request = tungstenite::Message::Text(
                        serde_json::json!({ "type": "tts-speak", "text": response.text })
                            .to_string()
                            .into(),
                    );
                    if let Err(error) = upstream_tx
                        .send(match wrap_for_upstream(tts_request, provider_rpc) {
                            Ok(message) => message,
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "failed to wrap tts-speak message"
                                );
                                return;
                            }
                        })
                        .await
                    {
                        tracing::warn!(error = %error, "failed to send tts-speak to runtime");
                        return;
                    }
                    vec![
                        client_text_message(&serde_json::json!({
                            "type": "full-text",
                            "text": response.text
                        })),
                        client_text_message(
                            &serde_json::json!({ "type": "conversation-chain-end" }),
                        ),
                    ]
                }
            }
            conversation::TurnOutcome::Cancelled => Vec::new(),
            conversation::TurnOutcome::Failed(error) => vec![
                client_text_message(&serde_json::json!({
                    "type": "error",
                    "message": format!("conversation failed: {error}")
                })),
                client_text_message(&serde_json::json!({"type": "conversation-chain-end"})),
            ],
        };
        for payload in payloads {
            if client_tx.send(payload).await.is_err() {
                return;
            }
        }
    }
}

fn client_text_message(value: &serde_json::Value) -> axum::extract::ws::Message {
    axum::extract::ws::Message::Text(value.to_string().into())
}

/// Executes one MCP tool call. Tool names may be `server.tool`; a bare name
/// is resolved by trying every connected server in order.
async fn execute_tool(
    registry: &tokio::sync::Mutex<mcp::McpRegistry>,
    tool_name: &str,
    arguments: serde_json::Value,
) -> String {
    let mut registry = registry.lock().await;
    if let Some((server_name, tool)) = tool_name.split_once('.') {
        if let Some(server) = registry.server(server_name) {
            let result = server
                .call_tool(
                    tool,
                    arguments.clone(),
                    &cancellation::CancellationToken::new(),
                )
                .await;
            return tool_outcome_text(tool_name, result);
        }
    }
    // Bare name: try every server; skip validation failures (tool not there).
    let names: Vec<String> = registry.servers().map(|(name, _)| name.clone()).collect();
    for server_name in names {
        let outcome = {
            let server = registry.server(&server_name);
            match server {
                Some(server) => {
                    server
                        .call_tool(
                            tool_name,
                            arguments.clone(),
                            &cancellation::CancellationToken::new(),
                        )
                        .await
                }
                None => continue,
            }
        };
        match outcome {
            Ok(result) => return tool_outcome_text(tool_name, Ok(result)),
            Err(mcp::McpError::UnknownTool(_)) => continue,
            Err(error) => return tool_outcome_text(tool_name, Err(error)),
        }
    }
    format!("tool '{tool_name}' not found on any MCP server")
}

/// Formats a tool result for the provider's `role=tool` message.
fn tool_outcome_text(tool: &str, result: Result<serde_json::Value, mcp::McpError>) -> String {
    match result {
        Ok(value) => {
            if let Some(content) = value.get("content").and_then(serde_json::Value::as_array) {
                let texts: Vec<String> = content
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .collect();
                if !texts.is_empty() {
                    return texts.join("\n");
                }
            }
            value.to_string()
        }
        Err(error) => format!("tool '{tool}' failed: {error}"),
    }
}

fn wrap_for_upstream(
    message: tungstenite::Message,
    provider_rpc: bool,
) -> Result<tungstenite::Message> {
    if provider_rpc {
        wrap_provider_message(message)
    } else {
        Ok(message)
    }
}

fn client_message_to_upstream(
    message: axum::extract::ws::Message,
    audio_state: &mut AudioSessionState,
) -> Result<Vec<tungstenite::Message>> {
    match message {
        axum::extract::ws::Message::Text(text) => {
            let message_type = serde_json::from_str::<serde_json::Value>(text.as_str())
                .ok()
                .and_then(|value| value.get("type")?.as_str().map(str::to_owned));
            match message_type.as_deref() {
                Some("audio-start") => {
                    let start: AudioStart = serde_json::from_str(text.as_str())?;
                    audio_state.start(&start)?;
                    Ok(Vec::new())
                }
                Some("audio-end") => {
                    let end: AudioEnd = serde_json::from_str(text.as_str())?;
                    let messages = audio_state.end()?;
                    Ok(attach_audio_end_metadata(messages, end.images))
                }
                _ => Ok(vec![to_upstream(axum::extract::ws::Message::Text(text))]),
            }
        }
        axum::extract::ws::Message::Binary(data) => audio_state.append_pcm16le(&data),
        other => Ok(vec![to_upstream(other)]),
    }
}

fn attach_audio_end_metadata(
    mut messages: Vec<tungstenite::Message>,
    images: Option<serde_json::Value>,
) -> Vec<tungstenite::Message> {
    let Some(images) = images else {
        return messages;
    };
    if let Some(message) = messages.iter_mut().rev().find(|message| {
        matches!(
            message,
            tungstenite::Message::Text(text)
                if text.as_str() == r#"{"type":"mic-audio-end"}"#
        )
    }) {
        *message = tungstenite::Message::Text(
            serde_json::json!({ "type": "mic-audio-end", "images": images })
                .to_string()
                .into(),
        );
    }
    messages
}

fn record_audio_output(metrics: &Metrics, message: &tungstenite::Message, input_is_binary: bool) {
    match message {
        tungstenite::Message::Binary(audio) => {
            metrics.audio_segments_total.fetch_add(1, Ordering::Relaxed);
            metrics
                .audio_samples_total
                .fetch_add((audio.len() / 2) as u64, Ordering::Relaxed);
        }
        tungstenite::Message::Text(text)
            if input_is_binary && text.as_str() == r#"{"type":"interrupt-signal","text":""}"# =>
        {
            metrics
                .vad_activations_total
                .fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn axum_message_size(message: &axum::extract::ws::Message) -> u64 {
    match message {
        axum::extract::ws::Message::Text(data) => data.len() as u64,
        axum::extract::ws::Message::Binary(data)
        | axum::extract::ws::Message::Ping(data)
        | axum::extract::ws::Message::Pong(data) => data.len() as u64,
        axum::extract::ws::Message::Close(_) => 0,
    }
}

fn tungstenite_message_size(message: &tungstenite::Message) -> u64 {
    match message {
        tungstenite::Message::Text(data) => data.len() as u64,
        tungstenite::Message::Binary(data)
        | tungstenite::Message::Ping(data)
        | tungstenite::Message::Pong(data) => data.len() as u64,
        tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => 0,
    }
}

fn to_upstream(message: axum::extract::ws::Message) -> tungstenite::Message {
    match message {
        axum::extract::ws::Message::Text(text) => tungstenite::Message::Text(text.as_str().into()),
        axum::extract::ws::Message::Binary(data) => tungstenite::Message::Binary(data),
        axum::extract::ws::Message::Ping(data) => tungstenite::Message::Ping(data),
        axum::extract::ws::Message::Pong(data) => tungstenite::Message::Pong(data),
        axum::extract::ws::Message::Close(frame) => {
            tungstenite::Message::Close(frame.map(|frame| tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.as_str().into(),
            }))
        }
    }
}

fn to_client(message: tungstenite::Message) -> axum::extract::ws::Message {
    match message {
        tungstenite::Message::Text(text) => axum::extract::ws::Message::Text(text.as_str().into()),
        tungstenite::Message::Binary(data) => axum::extract::ws::Message::Binary(data),
        tungstenite::Message::Ping(data) => axum::extract::ws::Message::Ping(data),
        tungstenite::Message::Pong(data) => axum::extract::ws::Message::Pong(data),
        tungstenite::Message::Close(frame) => {
            axum::extract::ws::Message::Close(frame.map(|frame| axum::extract::ws::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.as_str().into(),
            }))
        }
        tungstenite::Message::Frame(_) => {
            unreachable!("raw frames are not exposed by read streams")
        }
    }
}

async fn shutdown_signal(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
        changed = shutdown.changed() => { let _ = changed; },
    }

    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn spawn_echo_runtime() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(message) = websocket.next().await {
                let message = message.unwrap();
                let close = message.is_close();
                websocket.send(message).await.unwrap();
                if close {
                    break;
                }
            }
        });
        address
    }

    async fn spawn_http_runtime() -> SocketAddr {
        async fn echo(request: axum::extract::Request) -> Response {
            let (parts, body) = request.into_parts();
            let body = match to_bytes(body, 1024).await {
                Ok(body) => body,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };
            let payload = format!(
                "{} {} {}",
                parts.method,
                parts.uri,
                String::from_utf8_lossy(&body)
            );
            Response::builder()
                .status(StatusCode::CREATED)
                .header("x-runtime", "python")
                .body(Body::from(payload))
                .unwrap()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(any(echo)))
                .await
                .unwrap();
        });
        address
    }

    async fn spawn_slow_response_runtime(delay: Duration) -> SocketAddr {
        async fn slow_response(State(delay): State<Duration>) -> Response {
            let body = stream::once(async move {
                tokio::time::sleep(delay).await;
                Ok::<_, io::Error>(axum::body::Bytes::from_static(b"late response"))
            });
            Response::new(Body::from_stream(body))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(slow_response)).with_state(delay),
            )
            .await
            .unwrap();
        });
        address
    }

    async fn spawn_gateway(config: Config) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                build_router(&config)
                    .unwrap()
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        address
    }

    fn workspace_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(relative)
    }

    fn test_config() -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            python_ws_url: Url::parse("ws://127.0.0.1:12393/client-ws").unwrap(),
            max_connections: 2,
            max_connections_per_ip: 2,
            max_connection_attempts_per_minute: 60,
            max_message_bytes: 1024,
            max_http_body_bytes: 4096,
            http_upload_idle_timeout_ms: 100,
            http_response_idle_timeout_ms: 100,
            connect_timeout_ms: 100,
            session_queue_capacity: 4,
            max_transcript_entries: 200,
            chat_mode: ChatMode::Proxy,
            chat_provider: provider::ProviderKind::OpenAi,
            chat_base_url: None,
            chat_model: None,
            chat_system_prompt: None,
            chat_history_messages: 20,
            chat_provider_timeout_ms: 60_000,
            mcp_servers: Vec::new(),
            mcp_timeout_ms: 15_000,
            max_tool_rounds: 4,
            http_requests_per_minute_per_ip: 0,
            max_concurrent_http: 0,
            allow_missing_python: false,
            max_audio_seconds: 120,
            vad_rms_threshold: 0.1,
            vad_frame_samples: 4,
            vad_start_frames: 2,
            vad_end_frames: 2,
            vad_pre_roll_frames: 3,
            frontend_dir: workspace_path("frontend"),
            cache_dir: workspace_path("cache"),
            live2d_models_dir: workspace_path("live2d-models"),
            backgrounds_dir: workspace_path("backgrounds"),
            avatars_dir: workspace_path("avatars"),
            web_tool_dir: workspace_path("web_tool"),
            legacy_config_file: workspace_path("conf.yaml"),
            model_dict_file: workspace_path("model_dict.json"),
            legacy_characters_dir: workspace_path("characters"),
            settings_file: workspace_path("target/test-settings")
                .join(format!("{}.json", Uuid::new_v4())),
            export_settings_types: None,
            allowed_origins: vec!["http://localhost".parse().unwrap()],
        }
    }

    #[test]
    fn rejects_invalid_runtime_limits() {
        let mut config = test_config();
        config.vad_rms_threshold = f32::NAN;
        assert!(config.validate().is_err());

        config = test_config();
        config.vad_frame_samples = 0;
        assert!(config.validate().is_err());

        config = test_config();
        config.max_connections_per_ip = 0;
        assert!(config.validate().is_err());

        config = test_config();
        config.settings_file = PathBuf::new();
        assert!(config.validate().is_err());

        config = test_config();
        config.max_connections_per_ip = config.max_connections + 1;
        assert!(config.validate().is_err());

        config = test_config();
        config.http_upload_idle_timeout_ms = 0;
        assert!(config.validate().is_err());

        config = test_config();
        config.session_queue_capacity = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn limits_concurrent_connections_per_ip() {
        let limiter = Arc::new(PeerLimiter::new(1, 10));
        let peer = IpAddr::from([127, 0, 0, 1]);
        let permit = limiter.try_acquire(peer).unwrap();
        assert!(matches!(
            limiter.try_acquire(peer),
            Err(PeerLimitRejection::ConcurrentConnections)
        ));
        drop(permit);
        assert!(limiter.try_acquire(peer).is_ok());
    }

    #[test]
    fn limits_connection_attempts_per_ip_window() {
        let limiter = Arc::new(PeerLimiter::new(2, 2));
        let peer = IpAddr::from([127, 0, 0, 1]);
        drop(limiter.try_acquire(peer).unwrap());
        drop(limiter.try_acquire(peer).unwrap());
        assert!(matches!(
            limiter.try_acquire(peer),
            Err(PeerLimitRejection::ConnectionRate)
        ));
    }

    #[tokio::test]
    async fn serves_frontend_and_runtime_assets_without_python() {
        let app = build_router(&test_config()).unwrap();
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
        let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("location.host"));
        assert!(body.contains("/client-ws"));
        assert!(body.contains("./assets/"));

        let avatar = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/avatars/mao.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(avatar.status(), StatusCode::OK);
        assert_eq!(avatar.headers()[CONTENT_TYPE], "image/png");

        let metrics = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let metrics = to_bytes(metrics.into_body(), 4096).await.unwrap();
        let metrics = String::from_utf8(metrics.to_vec()).unwrap();
        assert!(metrics.contains("olv_gateway_http_requests_total 0\n"));
    }

    #[tokio::test]
    async fn health_endpoint_reports_ready() {
        let response = build_router(&test_config())
            .unwrap()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            body,
            r#"{"status":"ok","service":"open-llm-vtuber-gateway"}"#
        );
    }

    /// Mock upstream that accepts many connections and echoes every message.
    async fn spawn_echo_multi() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let Ok(mut websocket) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(message) = websocket.next().await {
                        let Ok(message) = message else {
                            break;
                        };
                        let close = message.is_close();
                        if websocket.send(message).await.is_err() {
                            break;
                        }
                        if close {
                            break;
                        }
                    }
                });
            }
        });
        address
    }

    /// Drives `clients` concurrent WebSocket sessions, each sending `rounds`
    /// text messages and awaiting the echo/full-text responses.
    async fn run_concurrent_clients(
        gateway: SocketAddr,
        clients: usize,
        rounds: usize,
        expect_messages: usize,
    ) {
        let mut handles = Vec::new();
        for index in 0..clients {
            handles.push(tokio::spawn(async move {
                let (mut websocket, _) = connect_async(format!("ws://{gateway}/client-ws"))
                    .await
                    .expect("client connect");
                for _round in 0..rounds {
                    websocket
                        .send(tungstenite::Message::Text(
                            format!(r#"{{"type":"ping-{index}"}}"#).into(),
                        ))
                        .await
                        .expect("client send");
                }
                let mut received = 0;
                while received < expect_messages {
                    let message = tokio::time::timeout(Duration::from_secs(10), websocket.next())
                        .await
                        .expect("client receive timeout")
                        .expect("client stream")
                        .expect("client stream error");
                    if let tungstenite::Message::Text(text) = message {
                        received += 1;
                        let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                        assert_eq!(value["type"], format!("ping-{index}"));
                    }
                }
                websocket.close(None).await.unwrap();
            }));
        }
        for handle in handles {
            tokio::time::timeout(Duration::from_secs(30), handle)
                .await
                .expect("concurrent client finished")
                .expect("concurrent client did not panic");
        }
    }

    #[tokio::test]
    async fn concurrent_proxy_sessions_serve_all_clients() {
        // Load test: 12 clients × 8 round trips through the echo upstream.
        let upstream = spawn_echo_multi().await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{upstream}/client-ws")).unwrap();
        config.max_connections = 64;
        config.max_connections_per_ip = 64;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&config, None, shutdown_tx, None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        run_concurrent_clients(gateway, 12, 8, 8).await;
    }

    #[tokio::test]
    async fn concurrent_native_sessions_answer_independently() {
        // Load test: native orchestration with several simultaneous clients;
        // every client receives its own full-text reply.
        let provider = spawn_native_provider().await;
        let mut config = native_test_config("127.0.0.1:1".parse().unwrap(), provider);
        config.max_connections = 64;
        config.max_connections_per_ip = 64;
        config.allow_missing_python = true;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&config, None, shutdown_tx, None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        let mut handles = Vec::new();
        for _ in 0..6 {
            handles.push(tokio::spawn(async move {
                let (mut websocket, _) = connect_async(format!("ws://{gateway}/client-ws"))
                    .await
                    .expect("client connect");
                websocket
                    .send(tungstenite::Message::Text(
                        r#"{"type":"text-input","text":"hello"}"#.into(),
                    ))
                    .await
                    .expect("client send");
                let mut received = Vec::new();
                while received.len() < 2 {
                    let message = tokio::time::timeout(Duration::from_secs(10), websocket.next())
                        .await
                        .expect("client receive timeout")
                        .expect("client stream")
                        .expect("client stream error");
                    if let tungstenite::Message::Text(text) = message {
                        let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                        let message_type = value["type"].as_str().unwrap_or_default();
                        if message_type == "set-model-and-conf" {
                            continue;
                        }
                        received.push(message_type.to_owned());
                    }
                }
                assert_eq!(received, vec!["full-text", "conversation-chain-end"]);
                websocket.close(None).await.unwrap();
            }));
        }
        for handle in handles {
            tokio::time::timeout(Duration::from_secs(30), handle)
                .await
                .expect("concurrent native client finished")
                .expect("concurrent native client did not panic");
        }
    }

    #[tokio::test]
    async fn shutdown_endpoint_triggers_graceful_shutdown() {
        let (shutdown_tx, mut shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&test_config(), None, shutdown_tx, None).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/shutdown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "shutting_down");

        // The graceful-shutdown future resolves.
        tokio::time::timeout(Duration::from_secs(2), shutdown_rx.changed())
            .await
            .expect("shutdown watch should be notified")
            .expect("shutdown watch should not be closed");
    }

    #[tokio::test]
    async fn auth_token_guards_management_endpoints_but_not_public_paths() {
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(
            &test_config(),
            None,
            shutdown_tx,
            Some("s3cret-token".to_owned()),
        )
        .unwrap();

        // Management API without a token is rejected.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/settings/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // With the bearer token it succeeds.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/settings/snapshot")
                    .header(AUTHORIZATION, "Bearer s3cret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // A wrong token is still rejected.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/shutdown")
                    .header(AUTHORIZATION, "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Health probes and browser assets stay public.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn http_rate_limit_rejects_excess_requests_per_ip() {
        let mut config = test_config();
        config.http_requests_per_minute_per_ip = 2;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&config, None, shutdown_tx, None).unwrap();

        let request_with_peer = || {
            let request = Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap();
            let (mut parts, body) = request.into_parts();
            parts
                .extensions
                .insert(ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    43210,
                ))));
            axum::http::Request::from_parts(parts, body)
        };

        for _ in 0..2 {
            let response = app.clone().oneshot(request_with_peer()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = app.oneshot(request_with_peer()).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn public_path_classification_covers_assets_and_websocket() {
        for path in [
            "/",
            "/healthz",
            "/assets/app.js",
            "/libs/lib.js",
            "/cache/file.wav",
            "/live2d-models/mao_pro/...",
            "/bg/wallpaper.png",
            "/avatars/mao.png",
            "/web-tool/index.html",
            "/client-ws",
        ] {
            assert!(is_public_path(path), "{path} should be public");
        }
        for path in [
            "/api/v1/settings/snapshot",
            "/shutdown",
            "/metrics",
            "/asr",
            "/docs",
        ] {
            assert!(!is_public_path(path), "{path} should be protected");
        }
    }

    #[tokio::test]
    async fn session_endpoint_serves_initial_snapshot_and_reset() {
        let app = build_router(&test_config()).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot["schema_version"], 1);
        assert_eq!(snapshot["phase"], "idle");
        assert_eq!(snapshot["turns"], 0);
        assert_eq!(snapshot["interrupts"], 0);
        assert_eq!(snapshot["transcript"], serde_json::json!([]));

        let reset = app
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/api/v1/session/reset")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
    }

    /// Mock upstream runtime: records every text message it receives and
    /// answers `asr-transcribe` with a canned transcription.
    async fn spawn_native_upstream(seen: Arc<std::sync::Mutex<Vec<String>>>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let Ok(mut websocket) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(message) = websocket.next().await {
                        let Ok(message) = message else {
                            break;
                        };
                        if message.is_close() {
                            break;
                        }
                        if let tungstenite::Message::Text(text) = &message {
                            let value: serde_json::Value = serde_json::from_str(text).unwrap();
                            let message_type =
                                value["type"].as_str().unwrap_or_default().to_owned();
                            seen.lock().unwrap().push(message_type.clone());
                            if message_type == "asr-transcribe" {
                                websocket
                                    .send(tungstenite::Message::Text(
                                        r#"{"type":"asr-result","text":"transcribed audio"}"#
                                            .into(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                        }
                    }
                });
            }
        });
        address
    }

    /// Mock OpenAI-compatible provider returning a canned streaming reply.
    async fn spawn_native_provider() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/chat/completions",
                    post(|_: axum::extract::Request| async {
                        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"native reply\"},\"index\":0}]}\n\n\
                                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}]}\n\n\
                                   data: [DONE]\n\n";
                        Response::builder()
                            .header("Content-Type", "text/event-stream")
                            .body(Body::from(body))
                            .unwrap()
                    }),
                ),
            )
            .await
            .unwrap();
        });
        address
    }

    fn native_test_config(upstream_addr: SocketAddr, provider_addr: SocketAddr) -> Config {
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{upstream_addr}/client-ws")).unwrap();
        config.chat_mode = ChatMode::Native;
        config.chat_provider = provider::ProviderKind::OpenAi;
        config.chat_base_url = Some(format!("http://{provider_addr}"));
        config.chat_model = Some("test-model".to_owned());
        config
    }

    /// Drives one WebSocket client conversation through the gateway.
    async fn run_native_client(
        gateway: SocketAddr,
        send: Vec<(bool, &str)>,
        expect_messages: usize,
        timeout_ms: u64,
    ) -> Vec<String> {
        let (mut websocket, _) = connect_async(format!("ws://{gateway}/client-ws"))
            .await
            .unwrap();
        let mut received = Vec::new();
        for (is_binary, payload) in send {
            if is_binary {
                websocket
                    .send(tungstenite::Message::Binary(
                        payload.as_bytes().to_vec().into(),
                    ))
                    .await
                    .unwrap();
            } else {
                websocket
                    .send(tungstenite::Message::Text(payload.to_owned().into()))
                    .await
                    .unwrap();
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        while received.len() < expect_messages {
            let message = tokio::time::timeout_at(deadline, websocket.next())
                .await
                .expect("client timed out waiting for gateway messages")
                .expect("client stream ended")
                .expect("client stream error");
            if let tungstenite::Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                let message_type = value["type"].as_str().unwrap_or_default();
                if message_type == "set-model-and-conf" {
                    // Native-mode initialization noise; not a conversation reply.
                    continue;
                }
                received.push(message_type.to_owned());
            }
        }
        websocket.close(None).await.unwrap();
        // Let the gateway finish tearing down the previous session before the
        // next client connects (upstream RSTs are otherwise observed as a
        // protocol error by the fresh connection).
        tokio::time::sleep(Duration::from_millis(100)).await;
        received
    }

    #[tokio::test]
    async fn native_chat_mode_routes_text_and_mic_turns() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let upstream = spawn_native_upstream(seen.clone()).await;
        let provider = spawn_native_provider().await;
        let config = native_test_config(upstream, provider);
        let app = build_router(&config).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // Text turn: the reply comes from the native provider.
        let received = run_native_client(
            gateway,
            vec![(false, r#"{"type":"text-input","text":"hello"}"#)],
            2,
            5_000,
        )
        .await;
        assert_eq!(received, vec!["full-text", "conversation-chain-end"]);

        // Mic turn: audio buffers, `audio-end` becomes `asr-transcribe`, the
        // mock upstream replies with a transcription, and the orchestrator
        // replies again (interrupting nothing).
        let received = run_native_client(
            gateway,
            vec![
                (false, r#"{"type":"audio-start","version":1,"encoding":"pcm_s16le","sample_rate":16000,"channels":1,"mode":"manual"}"#),
                (true, "binary-pcm-data!"),
                (false, r#"{"type":"audio-end","version":1}"#),
            ],
            2,
            5_000,
        )
        .await;
        assert_eq!(received, vec!["full-text", "conversation-chain-end"]);

        // The mock upstream saw asr-transcribe and tts-speak.
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().any(|t| t == "asr-transcribe"), "{seen:?}");
        assert!(
            seen.iter().any(|t| t == "tts-speak"),
            "expected tts-speak, got {seen:?}"
        );
        assert!(
            seen.iter().all(|t| t != "mic-audio-end"),
            "mic-audio-end must not reach the runtime in native mode, got {seen:?}"
        );
    }

    /// Mock provider that returns a tool call on the first round and a final
    /// text reply once the tool result is present in the conversation.
    async fn spawn_tool_provider() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/chat/completions",
                    post(|request: axum::extract::Request| async move {
                        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                            .await
                            .unwrap();
                        let payload: serde_json::Value =
                            serde_json::from_slice(&body).unwrap();
                        let has_tool_message = payload["messages"]
                            .as_array()
                            .is_some_and(|messages| {
                                messages
                                    .iter()
                                    .any(|m| m["role"] == "tool")
                            });
                        let body = if has_tool_message {
                            "data: {\"choices\":[{\"delta\":{\"content\":\"final answer\"},\"index\":0}]}\n\n\
                             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}]}\n\n\
                             data: [DONE]\n\n"
                        } else {
                            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\\\"weather\\\"}\"}}]},\"index\":0}]}\n\n\
                             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n\
                             data: [DONE]\n\n"
                        };
                        Response::builder()
                            .header("Content-Type", "text/event-stream")
                            .body(Body::from(body))
                            .unwrap()
                    }),
                ),
            )
            .await
            .unwrap();
        });
        address
    }

    #[tokio::test]
    async fn native_chat_mode_works_without_python_runtime() {
        // No mock upstream is started at all: the gateway must tolerate a
        // missing Python runtime when `--allow-missing-python` is set.
        let provider = spawn_native_provider().await;
        let mut config = native_test_config("127.0.0.1:1".parse().unwrap(), provider);
        config.allow_missing_python = true;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&config, None, shutdown_tx, None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // The connection succeeds and the native orchestrator answers.
        let received = run_native_client(
            gateway,
            vec![(false, r#"{"type":"text-input","text":"hello"}"#)],
            2,
            5_000,
        )
        .await;
        assert_eq!(received, vec!["full-text", "conversation-chain-end"]);

        // Without the flag the connection is rejected when Python is absent.
        let mut strict = native_test_config("127.0.0.1:1".parse().unwrap(), provider);
        strict.allow_missing_python = false;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(&strict, None, shutdown_tx, None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async(format!("ws://{gateway}/client-ws")),
        )
        .await
        .expect("strict-mode handshake must complete or fail promptly");
        if let Ok((mut websocket, _)) = result {
            // The handshake succeeded, but the gateway closes the session
            // immediately because the Python runtime is required.
            let next = tokio::time::timeout(Duration::from_secs(5), websocket.next())
                .await
                .expect("connection must close promptly");
            assert!(
                matches!(next, None | Some(Err(_))),
                "strict mode must not serve sessions without Python"
            );
        }
    }

    #[tokio::test]
    async fn native_chat_mode_executes_mcp_tools_in_agent_loop() {
        // In-memory MCP server with one `search` tool.
        let transport = mcp::InMemoryMcpTransport::new()
            .respond("initialize", serde_json::json!({}))
            .respond(
                "tools/list",
                serde_json::json!({
                    "tools": [{
                        "name": "search",
                        "description": "Search",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "q": { "type": "string" } }
                        }
                    }]
                }),
            )
            .respond(
                "tools/call",
                serde_json::json!({"content": [{"type": "text", "text": "sunny 25C"}]}),
            );
        let shared_calls = transport.calls();
        let mut registry = mcp::McpRegistry::new();
        let mut server = mcp::McpServer::new(
            "search-server".to_owned(),
            Box::new(transport),
            Duration::from_secs(2),
        );
        server
            .connect("2025-03-26", &cancellation::CancellationToken::new())
            .await
            .unwrap();
        registry.insert("search-server".to_owned(), server);

        let upstream = spawn_native_upstream(Arc::new(std::sync::Mutex::new(Vec::new()))).await;
        let provider = spawn_tool_provider().await;
        let mut config = native_test_config(upstream, provider);
        config.max_tool_rounds = 4;
        let (shutdown_tx, _shutdown_rx) = shutdown_channel();
        let app = build_router_with_mcp(
            &config,
            Some(Arc::new(tokio::sync::Mutex::new(registry))),
            shutdown_tx,
            None,
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });

        let received = run_native_client(
            gateway,
            vec![(
                false,
                r#"{"type":"text-input","text":"what is the weather?"}"#,
            )],
            2,
            5_000,
        )
        .await;
        assert_eq!(received, vec!["full-text", "conversation-chain-end"]);

        // The MCP tool was invoked exactly once with the model's arguments.
        let calls = shared_calls.lock().unwrap().clone();
        let tool_calls: Vec<_> = calls
            .iter()
            .filter(|(method, _)| method == "tools/call")
            .collect();
        assert_eq!(tool_calls.len(), 1, "{calls:?}");
        assert_eq!(tool_calls[0].1["name"], "search");
        assert_eq!(tool_calls[0].1["arguments"]["q"], "weather");
    }

    #[tokio::test]
    async fn capabilities_report_versioned_audio_protocol() {
        let response = build_router(&test_config())
            .unwrap()
            .oneshot(
                Request::builder()
                    .uri("/gateway/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let capabilities: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(capabilities["audio_protocol_version"], 1);
        assert_eq!(
            capabilities["audio_modes"],
            serde_json::json!(["manual", "energy-vad"])
        );
        assert_eq!(
            capabilities["audio_sample_rates"],
            serde_json::json!([16000, 44100, 48000])
        );
        assert_eq!(capabilities["max_connections_per_ip"], 2);
        assert_eq!(capabilities["max_connection_attempts_per_minute"], 60);
        assert_eq!(capabilities["http_upload_idle_timeout_ms"], 100);
        assert_eq!(capabilities["http_response_idle_timeout_ms"], 100);
        assert_eq!(capabilities["frontend_served_by_gateway"], true);
        assert_eq!(capabilities["settings_schema_version"], 1);
    }

    #[tokio::test]
    async fn settings_schema_is_served_without_python() {
        let response = build_router(&test_config())
            .unwrap()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/settings/schema")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let schema: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(schema["schemaVersion"], 1);
        assert_eq!(schema["schema"]["title"], "SettingsSnapshotV1");
        assert_eq!(schema["fields"].as_array().unwrap().len(), 21);
        assert!(schema["fields"].as_array().unwrap().iter().any(|field| {
            field["path"] == "client.connectionOverride.wsUrl"
                && field["applyEffect"] == "reconnect"
                && field["owner"] == "client"
        }));
    }

    #[tokio::test]
    async fn legacy_settings_are_served_redacted_and_without_python() {
        let root = workspace_path("target").join(format!("legacy-{}", Uuid::new_v4()));
        let characters = root.join("characters");
        fs::create_dir_all(&characters).unwrap();
        let config_file = root.join("conf.yaml");
        fs::write(
            &config_file,
            "character_config:\n  agent_config:\n    api_key: private-value\n",
        )
        .unwrap();
        fs::write(
            characters.join("alt.yaml"),
            "character_config:\n  conf_name: Alt\n",
        )
        .unwrap();
        let mut config = test_config();
        config.legacy_config_file = config_file;
        config.legacy_characters_dir = characters;

        let response = build_router(&config)
            .unwrap()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/settings/legacy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let legacy: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(legacy["schemaVersion"], 1);
        assert_eq!(legacy["available"], true);
        assert_eq!(legacy["characters"][0]["fileName"], "alt.yaml");
        assert_eq!(
            legacy["config"]["data"]["character_config"]["agent_config"]["api_key"],
            serde_json::json!({ "configured": true, "hint": "••••alue" })
        );
        assert!(
            !body
                .windows(b"private-value".len())
                .any(|window| window == b"private-value")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn settings_validation_is_local_and_side_effect_free() {
        let app = build_router(&test_config()).unwrap();
        let valid_snapshot = settings::SettingsSnapshotV1::default();
        let valid_response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/settings/validate")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&valid_snapshot).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(valid_response.status(), StatusCode::OK);
        let body = to_bytes(valid_response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"valid": true, "errors": []})
        );

        let mut invalid_snapshot = valid_snapshot;
        invalid_snapshot.client.appearance.locale = " ".to_owned();
        let invalid_response = app
            .oneshot(
                Request::post("/api/v1/settings/validate")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&invalid_snapshot).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_response.status(), StatusCode::OK);
        let body = to_bytes(invalid_response.into_body(), 4096).await.unwrap();
        let validation: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(validation["valid"], false);
        assert_eq!(validation["errors"][0]["path"], "client.appearance.locale");
        assert_eq!(validation["errors"][0]["code"], "required");
    }

    #[tokio::test]
    async fn settings_snapshot_patch_conflict_and_reload_are_consistent() {
        let config = test_config();
        let settings_file = config.settings_file.clone();
        let app = build_router(&config).unwrap();

        let snapshot_response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/settings/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        assert_eq!(
            snapshot_response.headers()[CACHE_CONTROL],
            HeaderValue::from_static("no-store")
        );
        let body = to_bytes(snapshot_response.into_body(), 4096).await.unwrap();
        let initial: settings::SettingsSnapshotV1 = serde_json::from_slice(&body).unwrap();
        assert_eq!(initial.revision, 0);
        assert!(!settings_file.exists());

        let mut client = initial.client;
        client.appearance.locale = "zh".to_owned();
        client.appearance.background_url = Some("https://example.com/bg.png".to_owned());
        client.connection_override = Some(settings::ConnectionOverride {
            ws_url: Some("wss://example.com/client-ws".to_owned()),
            base_url: Some("https://example.com".to_owned()),
        });
        let request = settings::SettingsPatchRequestV1 {
            base_revision: 0,
            client: client.clone(),
            provider: settings::ProviderPatchV1 {
                kind: settings::ProviderKindSetting::None,
                base_url: None,
                model: None,
                api_key: None,
            },
        };
        let applied_response = app
            .clone()
            .oneshot(
                Request::patch("/api/v1/settings")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(applied_response.status(), StatusCode::OK);
        let body = to_bytes(applied_response.into_body(), 8192).await.unwrap();
        let applied: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(applied["snapshot"]["revision"], 1);
        assert_eq!(
            applied["applyEffects"],
            serde_json::json!(["preview", "live", "reconnect"])
        );
        assert!(settings_file.exists());

        let conflict_response = app
            .oneshot(
                Request::patch("/api/v1/settings")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict_response.status(), StatusCode::CONFLICT);
        let body = to_bytes(conflict_response.into_body(), 8192).await.unwrap();
        let conflict: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(conflict["error"]["code"], "revision_conflict");
        assert_eq!(conflict["snapshot"]["revision"], 1);

        let reloaded = build_router(&config).unwrap();
        let response = reloaded
            .oneshot(
                Request::get("/api/v1/settings/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let persisted: settings::SettingsSnapshotV1 = serde_json::from_slice(&body).unwrap();
        assert_eq!(persisted.revision, 1);
        assert_eq!(persisted.client, client);
        fs::remove_file(&settings_file).unwrap();
    }

    #[tokio::test]
    async fn metrics_endpoint_reports_initial_counters() {
        let response = build_router(&test_config())
            .unwrap()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("olv_gateway_active_connections 0\n"));
        assert!(body.contains("olv_gateway_peer_limit_rejections_total 0\n"));
        assert!(body.contains("olv_gateway_http_upload_timeouts_total 0\n"));
        assert!(body.contains("olv_gateway_http_response_timeouts_total 0\n"));
        assert!(body.contains("olv_gateway_normalized_audio_samples_total 0\n"));
        assert!(body.contains("olv_gateway_audio_segments_total 0\n"));
    }

    #[tokio::test]
    async fn proxies_dynamic_model_info_before_static_directory() {
        let runtime_address = spawn_http_runtime().await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{runtime_address}/client-ws")).unwrap();
        let gateway_address = spawn_gateway(config).await;

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{gateway_address}/live2d-models/info"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.text().await.unwrap(), "GET /live2d-models/info ");
    }

    #[tokio::test]
    async fn proxies_http_requests_and_responses() {
        let runtime_address = spawn_http_runtime().await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{runtime_address}/client-ws")).unwrap();
        let gateway_address = spawn_gateway(config).await;

        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{gateway_address}/asr?language=en"))
            .header("x-client", "test")
            .body("audio")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-runtime"], "python");
        assert_eq!(
            response.text().await.unwrap(),
            "POST /asr?language=en audio"
        );
    }

    #[tokio::test]
    async fn times_out_idle_http_uploads() {
        let runtime_address = spawn_http_runtime().await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{runtime_address}/client-ws")).unwrap();
        config.http_upload_idle_timeout_ms = 10;
        let gateway_address = spawn_gateway(config).await;
        let delayed_upload = stream::once(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, io::Error>(axum::body::Bytes::from_static(b"late upload"))
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let response = client
            .post(format!("http://{gateway_address}/upload"))
            .body(reqwest::Body::wrap_stream(delayed_upload))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);

        let metrics = client
            .get(format!("http://{gateway_address}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(metrics.contains("olv_gateway_http_upload_timeouts_total 1\n"));
        assert!(metrics.contains("olv_gateway_http_response_timeouts_total 0\n"));
    }

    #[tokio::test]
    async fn times_out_idle_http_responses() {
        let runtime_address = spawn_slow_response_runtime(Duration::from_millis(50)).await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{runtime_address}/client-ws")).unwrap();
        config.http_response_idle_timeout_ms = 10;
        let gateway_address = spawn_gateway(config).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let response = client
            .get(format!("http://{gateway_address}/slow"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.bytes().await.is_err());

        let metrics = client
            .get(format!("http://{gateway_address}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(metrics.contains("olv_gateway_http_upload_timeouts_total 0\n"));
        assert!(metrics.contains("olv_gateway_http_response_timeouts_total 1\n"));
    }

    #[tokio::test]
    async fn rejects_second_concurrent_connection_from_same_ip() {
        let runtime_address = spawn_echo_runtime().await;
        let mut config = test_config();
        config.python_ws_url = Url::parse(&format!("ws://{runtime_address}/client-ws")).unwrap();
        config.max_connections_per_ip = 1;
        let gateway_address = spawn_gateway(config).await;

        let (mut first_client, _) = connect_async(format!("ws://{gateway_address}/client-ws"))
            .await
            .unwrap();
        let second_connection = connect_async(format!("ws://{gateway_address}/client-ws")).await;
        match second_connection {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(
                    response.status(),
                    tungstenite::http::StatusCode::TOO_MANY_REQUESTS
                );
                assert_eq!(response.headers()["retry-after"], "1");
            }
            Ok(_) => panic!("second connection unexpectedly bypassed the per-IP limit"),
            Err(error) => panic!("expected HTTP 429, received {error}"),
        }

        let metrics = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{gateway_address}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(metrics.contains("olv_gateway_peer_limit_rejections_total 1\n"));
        first_client.close(None).await.unwrap();
    }

    #[tokio::test]
    async fn proxies_text_and_binary_frames_end_to_end() {
        let runtime_address = spawn_echo_runtime().await;
        let mut config = test_config();
        config.python_ws_url =
            Url::parse(&format!("ws://{runtime_address}/internal/v1/session-ws")).unwrap();
        let gateway_address = spawn_gateway(config).await;

        let (mut client, _) = connect_async(format!("ws://{gateway_address}/client-ws"))
            .await
            .unwrap();

        client
            .send(tungstenite::Message::Text("hello runtime".into()))
            .await
            .unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            tungstenite::Message::Text("hello runtime".into())
        );

        client
            .send(tungstenite::Message::Text(
                r#"{"type":"audio-start","version":1,"encoding":"pcm_s16le","sample_rate":48000,"channels":1}"#
                    .into(),
            ))
            .await
            .unwrap();
        let source_audio = [0_i16, 0, 0, 16_384, 16_384, 16_384]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        client
            .send(tungstenite::Message::Binary(
                source_audio[..5].to_vec().into(),
            ))
            .await
            .unwrap();
        client
            .send(tungstenite::Message::Binary(
                source_audio[5..].to_vec().into(),
            ))
            .await
            .unwrap();
        client
            .send(tungstenite::Message::Text(r#"{"type":"audio-end"}"#.into()))
            .await
            .unwrap();

        let message = client.next().await.unwrap().unwrap();
        assert_eq!(
            message,
            tungstenite::Message::Binary(vec![0x00, 0x00, 0x00, 0x40].into())
        );

        assert_eq!(
            client.next().await.unwrap().unwrap(),
            tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into())
        );

        client
            .send(tungstenite::Message::Text(
                r#"{"type":"audio-start","version":1,"mode":"energy-vad","encoding":"pcm_s16le","sample_rate":16000,"channels":1}"#
                    .into(),
            ))
            .await
            .unwrap();
        let silence = pcm_frame(0);
        let voice = pcm_frame(16_384);
        let mut activation = silence.clone();
        activation.extend_from_slice(&voice);
        activation.extend_from_slice(&voice);
        client
            .send(tungstenite::Message::Binary(activation.clone().into()))
            .await
            .unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            tungstenite::Message::Text(r#"{"type":"interrupt-signal","text":""}"#.into())
        );

        let mut ending = voice;
        ending.extend_from_slice(&silence);
        ending.extend_from_slice(&silence);
        client
            .send(tungstenite::Message::Binary(ending.clone().into()))
            .await
            .unwrap();
        activation.extend_from_slice(&ending);
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            tungstenite::Message::Binary(activation.into())
        );
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into())
        );

        client
            .send(tungstenite::Message::Text(r#"{"type":"audio-end"}"#.into()))
            .await
            .unwrap();

        client.close(None).await.unwrap();

        let metrics_url = format!("http://{gateway_address}/metrics");
        let http_client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut metrics = String::new();
        for _ in 0..20 {
            metrics = http_client
                .get(&metrics_url)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            if metrics.contains("olv_gateway_active_connections 0\n") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(metrics.contains("olv_gateway_connections_total 1\n"));
        assert!(metrics.contains("olv_gateway_active_connections 0\n"));
        assert!(metrics.contains("olv_gateway_vad_activations_total 1\n"));
        assert!(metrics.contains("olv_gateway_normalized_audio_samples_total 26\n"));
        assert!(metrics.contains("olv_gateway_audio_segments_total 2\n"));
        assert!(metrics.contains("olv_gateway_audio_samples_total 26\n"));
    }

    fn test_energy_vad_config() -> EnergyVadConfig {
        EnergyVadConfig {
            rms_threshold: 0.1,
            frame_samples: 4,
            start_frames: 2,
            end_frames: 2,
            pre_roll_frames: 3,
        }
    }

    fn pcm_frame(sample: i16) -> Vec<u8> {
        (0..4).flat_map(|_| sample.to_le_bytes()).collect()
    }

    #[test]
    fn derives_python_http_origin_from_websocket_url() {
        assert_eq!(
            python_http_url(&Url::parse("ws://localhost:12393/client-ws").unwrap()).as_str(),
            "http://localhost:12393/"
        );
        assert_eq!(
            python_http_url(&Url::parse("wss://example.com/client-ws").unwrap()).as_str(),
            "https://example.com/"
        );
    }

    #[test]
    fn provider_rpc_envelopes_round_trip_text_and_binary() {
        let text = tungstenite::Message::Text(r#"{"type":"heartbeat"}"#.into());
        let wrapped_text = wrap_provider_message(text.clone()).unwrap();
        assert_ne!(wrapped_text, text);
        assert_eq!(unwrap_provider_message(wrapped_text).unwrap(), text);

        let binary = tungstenite::Message::Binary(vec![0, 1, 2, 3].into());
        let wrapped_binary = wrap_provider_message(binary.clone()).unwrap();
        assert_ne!(wrapped_binary, binary);
        assert_eq!(unwrap_provider_message(wrapped_binary).unwrap(), binary);
    }

    #[test]
    fn provider_rpc_rejects_unversioned_application_messages() {
        assert!(unwrap_provider_message(tungstenite::Message::Text("plain".into())).is_err());
        assert!(unwrap_provider_message(tungstenite::Message::Binary(vec![1, 2].into())).is_err());
        assert!(is_provider_rpc_url(
            &Url::parse("ws://127.0.0.1:12393/internal/v1/session-ws").unwrap()
        ));
        assert!(!is_provider_rpc_url(
            &Url::parse("ws://127.0.0.1:12393/client-ws").unwrap()
        ));
    }

    #[test]
    fn audio_end_metadata_is_forwarded_with_the_final_segment() {
        let mut state = AudioSessionState::new(16, test_energy_vad_config());
        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 16_000,
                channels: 1,
                mode: AudioMode::Manual,
            })
            .unwrap();
        state.append_pcm16le(&[0, 0, 1, 0]).unwrap();

        let messages = attach_audio_end_metadata(
            state.end().unwrap(),
            Some(serde_json::json!(["data:image/jpeg;base64,abc"])),
        );
        let end = messages.last().unwrap().to_text().unwrap();
        let end: serde_json::Value = serde_json::from_str(end).unwrap();

        assert_eq!(end["type"], "mic-audio-end");
        assert_eq!(
            end["images"],
            serde_json::json!(["data:image/jpeg;base64,abc"])
        );
    }

    #[test]
    fn empty_audio_segment_only_emits_end_trigger() {
        let mut state = AudioSessionState::new(1, test_energy_vad_config());
        let start = axum::extract::ws::Message::Text(
            r#"{"type":"audio-start","encoding":"pcm_s16le","sample_rate":16000,"channels":1}"#
                .into(),
        );
        assert!(
            client_message_to_upstream(start, &mut state)
                .unwrap()
                .is_empty()
        );
        let messages = client_message_to_upstream(
            axum::extract::ws::Message::Text(r#"{"type":"audio-end"}"#.into()),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            messages,
            vec![tungstenite::Message::Text(
                r#"{"type":"mic-audio-end"}"#.into()
            )]
        );
    }

    #[test]
    fn validates_audio_session_order_format_and_limit() {
        let mut state = AudioSessionState::new(2, test_energy_vad_config());
        assert!(state.append_pcm16le(&[0, 0]).is_err());
        assert!(
            state
                .start(&AudioStart {
                    version: 1,
                    encoding: "opus".to_owned(),
                    sample_rate: 48_000,
                    channels: 2,
                    mode: AudioMode::Manual,
                })
                .is_err()
        );
        assert!(
            state
                .start(&AudioStart {
                    version: 1,
                    encoding: "pcm_s16le".to_owned(),
                    sample_rate: 32_000,
                    channels: 1,
                    mode: AudioMode::Manual,
                })
                .is_err()
        );
        assert!(
            state
                .start(&AudioStart {
                    version: 2,
                    encoding: "pcm_s16le".to_owned(),
                    sample_rate: 16_000,
                    channels: 1,
                    mode: AudioMode::Manual,
                })
                .is_err()
        );

        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 16_000,
                channels: 1,
                mode: AudioMode::Manual,
            })
            .unwrap();
        state.append_pcm16le(&[0, 0, 1, 0]).unwrap();
        assert!(state.append_pcm16le(&[2, 0]).is_err());
        assert_eq!(
            state.end().unwrap(),
            vec![
                tungstenite::Message::Binary(vec![0, 0, 1, 0].into()),
                tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into()),
            ]
        );
        assert!(state.end().is_err());
    }

    #[test]
    fn resamples_48khz_pcm_across_arbitrary_chunks() {
        let mut state = AudioSessionState::new(10, test_energy_vad_config());
        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 48_000,
                channels: 1,
                mode: AudioMode::Manual,
            })
            .unwrap();

        let source_samples = [300_i16, 600, 900, -300, -600, -900, 300, 600];
        let source_bytes = source_samples
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(state.append_pcm16le(&source_bytes[..1]).unwrap().is_empty());
        assert!(
            state
                .append_pcm16le(&source_bytes[1..7])
                .unwrap()
                .is_empty()
        );
        assert!(state.append_pcm16le(&source_bytes[7..]).unwrap().is_empty());

        let expected = [600_i16, -600, 450]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            state.end().unwrap(),
            vec![
                tungstenite::Message::Binary(expected.into()),
                tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into()),
            ]
        );
    }

    #[test]
    fn resamples_44100hz_without_clock_drift() {
        let mut normalizer = Pcm16Normalizer::new(44_100).unwrap();
        let source = (0..441)
            .flat_map(|_| 1_200_i16.to_le_bytes())
            .collect::<Vec<_>>();
        let mut normalized = normalizer.push(&source[..317]);
        normalized.extend(normalizer.push(&source[317..]));
        normalized.extend(normalizer.finish().unwrap());

        let samples = normalized
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 160);
        assert!(samples.iter().all(|sample| *sample == 1_200));
    }

    #[test]
    fn energy_vad_frames_across_chunks_and_emits_segment() {
        let mut state = AudioSessionState::new(100, test_energy_vad_config());
        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 16_000,
                channels: 1,
                mode: AudioMode::EnergyVad,
            })
            .unwrap();

        let silence = pcm_frame(0);
        let voice = pcm_frame(16_384);
        let mut initial = silence.clone();
        initial.extend_from_slice(&voice[..2]);
        assert!(state.append_pcm16le(&initial).unwrap().is_empty());

        let mut activation = voice[2..].to_vec();
        activation.extend_from_slice(&voice);
        assert_eq!(
            state.append_pcm16le(&activation).unwrap(),
            vec![tungstenite::Message::Text(
                r#"{"type":"interrupt-signal","text":""}"#.into()
            )]
        );

        let mut ending = voice.clone();
        ending.extend_from_slice(&silence);
        ending.extend_from_slice(&silence);
        let messages = state.append_pcm16le(&ending).unwrap();
        let mut expected_audio = silence;
        expected_audio.extend_from_slice(&voice);
        expected_audio.extend_from_slice(&voice);
        expected_audio.extend_from_slice(&ending);
        assert_eq!(
            messages,
            vec![
                tungstenite::Message::Binary(expected_audio.into()),
                tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into()),
            ]
        );
        assert!(state.end().unwrap().is_empty());
    }

    #[test]
    fn energy_vad_flushes_active_speech_on_audio_end() {
        let mut state = AudioSessionState::new(100, test_energy_vad_config());
        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 16_000,
                channels: 1,
                mode: AudioMode::EnergyVad,
            })
            .unwrap();
        let mut voice = pcm_frame(16_384);
        voice.extend_from_slice(&pcm_frame(16_384));
        assert_eq!(state.append_pcm16le(&voice).unwrap().len(), 1);
        assert_eq!(state.end().unwrap().len(), 2);
    }

    #[test]
    fn validates_websocket_origins() {
        let allowed_origins = vec![HeaderValue::from_static("http://localhost:12393")];
        let mut headers = HeaderMap::new();
        assert!(origin_is_allowed(&headers, &allowed_origins));

        headers.insert(ORIGIN, HeaderValue::from_static("https://attacker.example"));
        assert!(!origin_is_allowed(&headers, &allowed_origins));

        headers.insert(ORIGIN, HeaderValue::from_static("http://localhost:12393"));
        assert!(origin_is_allowed(&headers, &allowed_origins));
    }

    #[test]
    fn rejects_incomplete_pcm16le_sample_at_stream_end() {
        let mut state = AudioSessionState::new(1, test_energy_vad_config());
        state
            .start(&AudioStart {
                version: 1,
                encoding: "pcm_s16le".to_owned(),
                sample_rate: 16_000,
                channels: 1,
                mode: AudioMode::Manual,
            })
            .unwrap();
        assert!(state.append_pcm16le(&[0x00]).unwrap().is_empty());
        assert!(state.end().is_err());

        assert!(state.append_pcm16le(&[0x00]).unwrap().is_empty());
        assert_eq!(
            state.end().unwrap(),
            vec![
                tungstenite::Message::Binary(vec![0, 0].into()),
                tungstenite::Message::Text(r#"{"type":"mic-audio-end"}"#.into()),
            ]
        );
    }

    #[test]
    fn preserves_text_and_server_binary_messages() {
        let text = axum::extract::ws::Message::Text("hello".into());
        assert_eq!(
            to_upstream(text),
            tungstenite::Message::Text("hello".into())
        );

        let binary = tungstenite::Message::Binary(vec![1, 2, 3].into());
        assert_eq!(
            to_client(binary),
            axum::extract::ws::Message::Binary(vec![1, 2, 3].into())
        );
    }
}
