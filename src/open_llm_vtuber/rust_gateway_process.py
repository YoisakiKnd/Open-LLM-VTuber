import asyncio
from http.client import HTTPConnection
from pathlib import Path
import subprocess
import time

from loguru import logger

from .config_manager.system import RustGatewayConfig


class RustGatewayProcess:
    def __init__(self, config: RustGatewayConfig, python_host: str, python_port: int):
        self.config = config
        self.python_host = "127.0.0.1" if python_host == "0.0.0.0" else python_host
        self.python_port = python_port
        self.process: subprocess.Popen | None = None

    @property
    def health_url(self) -> str:
        health_host = "127.0.0.1" if self.config.host == "0.0.0.0" else self.config.host
        return f"http://{health_host}:{self.config.port}/healthz"

    def start(self) -> None:
        binary = Path(self.config.binary_path)
        if not binary.is_file():
            raise FileNotFoundError(
                f"Rust gateway binary not found: {binary}. "
                "Build it with `cargo build --manifest-path rust-gateway/Cargo.toml --release`."
            )

        command = self._build_command(binary)
        self.process = subprocess.Popen(command)
        logger.info(
            f"Starting Rust gateway on {self.config.host}:{self.config.port}"
        )

    def _build_command(self, binary: Path) -> list[str]:
        return [
            str(binary),
            "--listen",
            f"{self.config.host}:{self.config.port}",
            "--python-ws-url",
            f"ws://{self.python_host}:{self.python_port}/internal/v1/session-ws",
            "--settings-file",
            self.config.settings_file,
            "--legacy-config-file",
            self.config.legacy_config_file,
            "--legacy-characters-dir",
            self.config.legacy_characters_dir,
            "--max-connections",
            str(self.config.max_connections),
            "--max-connections-per-ip",
            str(self.config.max_connections_per_ip),
            "--max-connection-attempts-per-minute",
            str(self.config.max_connection_attempts_per_minute),
            "--max-message-bytes",
            str(self.config.max_message_bytes),
            "--max-http-body-bytes",
            str(self.config.max_http_body_bytes),
            "--http-upload-idle-timeout-ms",
            str(self.config.http_upload_idle_timeout_ms),
            "--http-response-idle-timeout-ms",
            str(self.config.http_response_idle_timeout_ms),
            "--session-queue-capacity",
            str(self.config.session_queue_capacity),
            "--max-audio-seconds",
            str(self.config.max_audio_seconds),
            "--vad-rms-threshold",
            str(self.config.vad_rms_threshold),
            "--vad-frame-samples",
            str(self.config.vad_frame_samples),
            "--vad-start-frames",
            str(self.config.vad_start_frames),
            "--vad-end-frames",
            str(self.config.vad_end_frames),
            "--vad-pre-roll-frames",
            str(self.config.vad_pre_roll_frames),
            "--connect-timeout-ms",
            str(self.config.connect_timeout_ms),
            "--allowed-origins",
            ",".join(self.config.allowed_origins),
        ]

    async def wait_until_ready(self) -> None:
        deadline = time.monotonic() + self.config.startup_timeout_seconds
        while time.monotonic() < deadline:
            if self.process and self.process.poll() is not None:
                raise RuntimeError(
                    f"Rust gateway exited with code {self.process.returncode}"
                )
            try:
                await asyncio.to_thread(self._check_health)
                logger.info(f"Rust gateway ready at {self.health_url}")
                return
            except OSError:
                await asyncio.sleep(0.1)
        raise TimeoutError(f"Rust gateway did not become ready at {self.health_url}")

    def _check_health(self) -> None:
        health_host = (
            "127.0.0.1" if self.config.host == "0.0.0.0" else self.config.host
        )
        connection = HTTPConnection(health_host, self.config.port, timeout=0.5)
        try:
            connection.request("GET", "/healthz")
            response = connection.getresponse()
            if response.status != 200:
                raise OSError(f"Rust gateway health returned {response.status}")
        finally:
            connection.close()

    def stop(self) -> None:
        if not self.process or self.process.poll() is not None:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2)
        logger.info("Rust gateway stopped")
