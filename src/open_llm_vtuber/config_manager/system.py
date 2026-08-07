# config_manager/system.py
from pydantic import Field, model_validator
from typing import Dict, ClassVar, List
from .i18n import I18nMixin, Description


class RustGatewayConfig(I18nMixin):
    enabled: bool = Field(False, alias="enabled")
    binary_path: str = Field(
        "rust-gateway/target/release/open-llm-vtuber-gateway",
        alias="binary_path",
    )
    settings_file: str = Field(
        ".olv/settings.v1.json", alias="settings_file", min_length=1
    )
    legacy_config_file: str = Field("conf.yaml", alias="legacy_config_file", min_length=1)
    legacy_characters_dir: str = Field(
        "characters", alias="legacy_characters_dir", min_length=1
    )
    host: str = Field("127.0.0.1", alias="host")
    port: int = Field(12394, alias="port")
    max_connections: int = Field(64, alias="max_connections", ge=1)
    max_connections_per_ip: int = Field(
        8, alias="max_connections_per_ip", ge=1
    )
    max_connection_attempts_per_minute: int = Field(
        60, alias="max_connection_attempts_per_minute", ge=1
    )
    max_message_bytes: int = Field(
        2 * 1024 * 1024, alias="max_message_bytes", ge=1024
    )
    max_http_body_bytes: int = Field(
        32 * 1024 * 1024, alias="max_http_body_bytes", ge=1024
    )
    http_upload_idle_timeout_ms: int = Field(
        15_000, alias="http_upload_idle_timeout_ms", ge=1
    )
    http_response_idle_timeout_ms: int = Field(
        60_000, alias="http_response_idle_timeout_ms", ge=1
    )
    session_queue_capacity: int = Field(32, alias="session_queue_capacity", ge=1)
    max_audio_seconds: int = Field(120, alias="max_audio_seconds", ge=1)
    vad_rms_threshold: float = Field(
        0.015, alias="vad_rms_threshold", gt=0, le=1
    )
    vad_frame_samples: int = Field(512, alias="vad_frame_samples", ge=1)
    vad_start_frames: int = Field(3, alias="vad_start_frames", ge=1)
    vad_end_frames: int = Field(24, alias="vad_end_frames", ge=1)
    vad_pre_roll_frames: int = Field(10, alias="vad_pre_roll_frames", ge=0)
    connect_timeout_ms: int = Field(5000, alias="connect_timeout_ms", ge=1)
    startup_timeout_seconds: float = Field(
        5.0, alias="startup_timeout_seconds", gt=0
    )
    allowed_origins: List[str] = Field(
        default_factory=lambda: [
            "http://localhost:12394",
            "http://127.0.0.1:12394",
        ],
        alias="allowed_origins",
    )

    @model_validator(mode="after")
    def check_port(self):
        if self.port < 0 or self.port > 65535:
            raise ValueError("Rust gateway port must be between 0 and 65535")
        if self.max_connections_per_ip > self.max_connections:
            raise ValueError(
                "Per-IP Rust gateway connection limit cannot exceed the global limit"
            )
        return self


class SystemConfig(I18nMixin):
    """System configuration settings."""

    conf_version: str = Field(..., alias="conf_version")
    host: str = Field(..., alias="host")
    port: int = Field(..., alias="port")
    config_alts_dir: str = Field(..., alias="config_alts_dir")
    tool_prompts: Dict[str, str] = Field(..., alias="tool_prompts")
    enable_proxy: bool = Field(False, alias="enable_proxy")
    rust_gateway: RustGatewayConfig = Field(
        default_factory=RustGatewayConfig, alias="rust_gateway"
    )

    DESCRIPTIONS: ClassVar[Dict[str, Description]] = {
        "conf_version": Description(en="Configuration version", zh="配置文件版本"),
        "host": Description(en="Server host address", zh="服务器主机地址"),
        "port": Description(en="Server port number", zh="服务器端口号"),
        "config_alts_dir": Description(
            en="Directory for alternative configurations", zh="备用配置目录"
        ),
        "tool_prompts": Description(
            en="Tool prompts to be inserted into persona prompt",
            zh="要插入到角色提示词中的工具提示词",
        ),
        "enable_proxy": Description(
            en="Enable proxy mode for multiple clients",
            zh="启用代理模式以支持多个客户端使用一个 ws 连接",
        ),
        "rust_gateway": Description(
            en="Rust realtime WebSocket gateway settings",
            zh="Rust 实时 WebSocket 网关设置",
        ),
    }

    @model_validator(mode="after")
    def check_port(cls, values):
        port = values.port
        if port < 0 or port > 65535:
            raise ValueError("Port must be between 0 and 65535")
        return values
