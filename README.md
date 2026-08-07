# Open-LLM-VTuber

一个带 Live2D 虚拟形象、支持语音交互的本地 AI 桌宠/桌面应用。基于上游
[Open-LLM-VTuber](https://github.com/Open-LLM-VTuber/Open-LLM-VTuber) 重构：
**Rust 网关作为统一业务主程序**（会话编排、LLM Provider、MCP 工具、设置与密钥管理），
Electron 仅作原生窗口壳与运行时监督者，Python 降为**可选**的 ASR/TTS 兼容 sidecar。

目标平台：**Windows / Linux / macOS 三平台一致**（macOS 为本机开发验证环境）。

---

## 架构

```
┌────────────────────────────────────────────────────────────┐
│ Electron（窗口壳 + RuntimeSupervisor 监督）                  │
│   ├── Renderer（React + Chakra UI + Live2D）               │
│   └── Main（窗口管理、桌宠模式、进程监督、类型化 Preload API）│
└──────────────────────────┬─────────────────────────────────┘
                           │ WebSocket (/client-ws) + HTTP
┌──────────────────────────▼─────────────────────────────────┐
│ Rust Gateway（open-llm-vtuber-gateway）                     │
│   ├── SessionActor：会话相位监督、音频归一化（PCM16/16kHz）   │
│   ├── 原生编排（--chat-mode native，默认）：                 │
│   │   文本/语音输入 → ChatSession → Provider → 工具循环 →    │
│   │   full-text + tts-speak（语音可选）                     │
│   ├── Provider：OpenAI-compatible / Anthropic / Ollama      │
│   ├── MCP 客户端：stdio JSON-RPC、工具注册/校验/超时/取消     │
│   ├── Settings：设置域（乐观锁 revision、密钥脱敏掩码）       │
│   ├── Legacy 适配器：只读 YAML 镜像 + 递归密钥脱敏            │
│   └── 安全：可选 Bearer 鉴权、每 IP 限流、并发上限、/shutdown │
└──────────┬──────────────────────────────────────────────────┘
           │ 可选（OLV_DESKTOP_START_PYTHON=1）
┌──────────▼──────────────────────────────────────────────────┐
│ Python sidecar（ASR/TTS 兼容层，默认关闭）                    │
│   /internal/v1/session-ws（loopback，OLV-RPC envelope）      │
└─────────────────────────────────────────────────────────────┘
```

- **默认路径完全 Rust**：不启动 Python 即可完成对话（LLM + MCP 工具 + 设置全在 Rust）。
- **密钥边界**：API Key 仅存 Rust 侧 `secrets.v1.json`（本机 userData），快照/UI 只出
  `{configured, hint}` 掩码；Token 只从环境变量读取，绝不出现在进程命令行。
- **协议兼容**：`--chat-mode proxy` 可回退为经 Python 的旧对话链路；V1 音频协议
  （`audio-start` → PCM16 分块 → `audio-end`）与旧 JSON 音频协议并存。

## 快速开始

### 桌面应用（推荐）

```bash
cd apps/desktop
npm ci
npm run prepare:runtime   # 拷贝 Rust release 二进制到 resources/runtime
npm run build             # electron-vite 构建
npm run build:unpack      # electron-builder --dir（本机解包产物）
```

启动后由 `RuntimeSupervisor` 自动拉起 Rust 网关并等待 `/healthz`；
关闭时先 `POST /shutdown` 优雅回收（全平台），失败再强制终止。

可选：`OLV_DESKTOP_START_PYTHON=1` 启动 Python sidecar（ASR/TTS 语音链路）。

### 纯 Rust 网关（无桌面壳）

```bash
cargo build --manifest-path rust-gateway/Cargo.toml --release --locked
OLV_PROVIDER_OPENAI_API_KEY=sk-... ./rust-gateway/target/release/open-llm-vtuber-gateway \
  --chat-mode native \
  --chat-provider openai \
  --chat-base-url https://api.openai.com/v1 \
  --chat-model gpt-4o-mini \
  --allow-missing-python
```

浏览器访问 `http://127.0.0.1:12394`（网关静态托管前端）。

### Web 模式（旧链路）

```bash
OLV_SKIP_RUST_GATEWAY=1 PYTHONPATH=src uv run --project . python run_server.py
```

## 配置

### 网关 CLI / 环境变量（关键项）

| 参数 | 环境变量 | 默认 | 说明 |
|---|---|---|---|
| `--listen` | `OLV_GATEWAY_LISTEN` | `127.0.0.1:12394` | 监听地址 |
| `--chat-mode` | `OLV_GATEWAY_CHAT_MODE` | `proxy` | `native` 启用 Rust 原生编排 |
| `--chat-provider` | `OLV_GATEWAY_CHAT_PROVIDER` | `openai` | `openai`/`anthropic`/`ollama` |
| `--chat-base-url` | `OLV_GATEWAY_CHAT_BASE_URL` | 按 provider | Provider 端点 |
| `--mcp-server` | `OLV_GATEWAY_MCP_SERVERS` | — | `name=command args`（可重复，`;` 分隔） |
| `--allow-missing-python` | `OLV_GATEWAY_ALLOW_MISSING_PYTHON` | 关 | 无 Python 时仍接受会话 |
| `--http-requests-per-minute-per-ip` | `OLV_GATEWAY_HTTP_REQUESTS_PER_MINUTE_PER_IP` | 0 | 每 IP 限流（0=不限） |
| — | `OLV_GATEWAY_AUTH_TOKEN` | — | 管理端点 Bearer 鉴权（仅环境变量） |
| — | `OLV_PROVIDER_OPENAI_API_KEY` | — | OpenAI API Key（仅环境变量） |
| — | `OLV_PROVIDER_ANTHROPIC_API_KEY` | — | Anthropic API Key（仅环境变量） |

Provider 配置也可经设置 API 管理（`/api/v1/settings` 的 `provider` 域，密钥只回掩码）。

### 设置域与密钥

- 设置文件：`userData/settings.v1.json`（revision 乐观锁、原子写）。
- 密钥文件：`userData/secrets.v1.json`（明文仅本机；快照/API 只出掩码）。
- 旧配置：`conf.yaml` 与 `characters/` 经 Legacy 适配器**只读**镜像（递归脱敏）。

## 安全

- 鉴权：可选 Bearer Token（`OLV_GATEWAY_AUTH_TOKEN`）保护管理/代理端点；
  浏览器静态资源与 `/healthz`、`/client-ws` 保持公开（浏览器 WebSocket 无法携带
  自定义请求头）。
- 限流：HTTP 每真实对端 IP 限流（读 TCP 对端，不信任 `X-Forwarded-For`）；
  可选全局 HTTP 并发上限。
- 监听默认仅回环；对外暴露时请使用反向代理并自行配置传输层鉴权。
- 密钥/Token 只经环境变量或 Rust 设置存储注入，绝不进进程命令行。

## 开发

```bash
# Rust 网关
cargo test --manifest-path rust-gateway/Cargo.toml --locked
cargo clippy --manifest-path rust-gateway/Cargo.toml --all-targets --all-features -- -D warnings

# Python 兼容层
PYTHONPATH=src uv run --no-project --with ruff ruff check src tests run_server.py
PYTHONPATH=src uv run --no-project python -m unittest discover -s tests

# 桌面（Vitest + 类型检查 + 构建）
cd apps/desktop && npm test && npm run typecheck:settings && npm run typecheck:node
```

设置协议与 TypeScript 类型由 Rust Schema 生成，生成文件不可手改：
`cargo run --manifest-path rust-gateway/Cargo.toml -- --export-settings-types apps/desktop/src/renderer/src/settings/generated/settings-v1.generated.ts`

## 目录

| 路径 | 说明 |
|---|---|
| `rust-gateway/` | Rust 网关（会话/编排/Provider/MCP/设置/安全） |
| `apps/desktop/` | Electron + React 桌面应用（含 `UPSTREAM.md` 上游来源说明） |
| `src/open_llm_vtuber/` | Python 兼容层（可选 sidecar） |
| `frontend/`、`live2d-models/`、`backgrounds/`、`avatars/`、`characters/`、`web_tool/` | 运行资源 |
| `config_templates/` | 配置模板 |
| `PROJECT_PLAN.md` | 重构完成计划与里程碑跟踪 |

## 许可证

- 上游代码遵循 Open-LLM-VTuber 的 [LICENSE](LICENSE)（含商业附加条件，分发前请法律审查）。
- Live2D Cubism 组件受 [LICENSE-Live2D.md](LICENSE-Live2D.md) 专有许可约束。
