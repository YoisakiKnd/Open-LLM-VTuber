import tempfile
import unittest
from pathlib import Path

from src.open_llm_vtuber.config_manager.system import RustGatewayConfig
from src.open_llm_vtuber.config_manager.utils import read_yaml, validate_config
from src.open_llm_vtuber.rust_gateway_process import RustGatewayProcess


class RustGatewayProcessTests(unittest.TestCase):
    def test_missing_binary_has_actionable_error(self):
        with tempfile.TemporaryDirectory() as directory:
            config = RustGatewayConfig(
                binary_path=str(Path(directory) / "missing-gateway")
            )
            gateway = RustGatewayProcess(config, "localhost", 12393)

            with self.assertRaisesRegex(FileNotFoundError, "cargo build"):
                gateway.start()

    def test_energy_vad_arguments_are_forwarded(self):
        config = RustGatewayConfig(
            settings_file="state/settings.v1.json",
            max_connections_per_ip=3,
            max_connection_attempts_per_minute=12,
            http_upload_idle_timeout_ms=2500,
            http_response_idle_timeout_ms=9000,
            vad_rms_threshold=0.02,
            vad_frame_samples=256,
            vad_start_frames=4,
            vad_end_frames=20,
            vad_pre_roll_frames=8,
        )
        gateway = RustGatewayProcess(config, "127.0.0.1", 12393)

        command = gateway._build_command(Path("gateway"))
        expected = {
            "--settings-file": "state/settings.v1.json",
            "--legacy-config-file": "conf.yaml",
            "--legacy-characters-dir": "characters",
            "--max-connections-per-ip": "3",
            "--max-connection-attempts-per-minute": "12",
            "--http-upload-idle-timeout-ms": "2500",
            "--http-response-idle-timeout-ms": "9000",
            "--vad-rms-threshold": "0.02",
            "--vad-frame-samples": "256",
            "--vad-start-frames": "4",
            "--vad-end-frames": "20",
            "--vad-pre-roll-frames": "8",
        }
        for argument, value in expected.items():
            self.assertEqual(command[command.index(argument) + 1], value)

    def test_default_templates_include_energy_vad_settings(self):
        for path in (
            "config_templates/conf.default.yaml",
            "config_templates/conf.ZH.default.yaml",
        ):
            with self.subTest(path=path):
                config = validate_config(read_yaml(path)).system_config.rust_gateway
                self.assertEqual(config.settings_file, ".olv/settings.v1.json")
                self.assertEqual(config.legacy_config_file, "conf.yaml")
                self.assertEqual(config.legacy_characters_dir, "characters")
                self.assertEqual(config.max_connections_per_ip, 8)
                self.assertEqual(config.max_connection_attempts_per_minute, 60)
                self.assertEqual(config.http_upload_idle_timeout_ms, 15_000)
                self.assertEqual(config.http_response_idle_timeout_ms, 60_000)
                self.assertEqual(config.vad_rms_threshold, 0.015)
                self.assertEqual(config.vad_frame_samples, 512)
                self.assertEqual(config.vad_start_frames, 3)
                self.assertEqual(config.vad_end_frames, 24)
                self.assertEqual(config.vad_pre_roll_frames, 10)

    def test_empty_paths_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "at least 1 character"):
            RustGatewayConfig(legacy_config_file="")
        with self.assertRaisesRegex(ValueError, "at least 1 character"):
            RustGatewayConfig(legacy_characters_dir="")
        with self.assertRaisesRegex(ValueError, "at least 1 character"):
            RustGatewayConfig(settings_file="")

    def test_per_ip_limit_cannot_exceed_global_limit(self):
        with self.assertRaisesRegex(ValueError, "cannot exceed the global limit"):
            RustGatewayConfig(max_connections=2, max_connections_per_ip=3)

    def test_zero_bind_hosts_use_loopback_for_internal_connections(self):
        config = RustGatewayConfig(host="0.0.0.0", port=12394)
        gateway = RustGatewayProcess(config, "0.0.0.0", 12393)

        self.assertEqual(gateway.python_host, "127.0.0.1")
        self.assertEqual(gateway.health_url, "http://127.0.0.1:12394/healthz")


if __name__ == "__main__":
    unittest.main()
