# Open-LLM-VTuber 重构完成计划（PROJECT PLAN）

> 目标：将 Open-LLM-VTuber 重构为 **Rust 唯一业务主程序 + React/Electron 完整桌面应用**，**全平台（Windows / Linux / macOS）行为一致**，macOS 仅为本机开发验证环境。
> 当前整体进度：约 **55%–60%**（里程碑见文末跟踪表）。本计划每阶段均给出可验证的验收标准。

---

## 总览与原则

| 原则 | 说明 |
|---|---|
| 兼容过渡 | 阶段 1–4 必须兼容现有 `/client-ws` 文本协议与旧 JSON 音频客户端；`system_config.rust_gateway.enabled: false` 可回退 |
| 密钥边界 | Token/密钥只存 Rust，React 只能获得 `configured` + 脱敏提示，绝不进响应体、不进进程 CLI 参数 |
| 类型单一来源 | 设置协议与 TypeScript 类型由 Rust Schema 生成，生成文件不可手改 |
| 原子与事务 | 设置乐观锁原子提交；预览只写内存；Save/Cancel 一致事务语义 |
| 全平台 | 路径分隔、可执行名（`.exe`）、进程信号（Windows 无 SIGTERM → `taskkill`）、venv 目录（`Scripts` vs `bin`）等一律按平台分支；CI 三平台矩阵 |
| 不提交 | 过程不执行 Git commit/分支；`release/`、`dist/`、`out/`、`resources/runtime/` 仅构建产物 |
| 回滚 | 旧键镜像 + `legacy.extra` + 两个稳定版本回滚窗口，之后才允许移除旧读取逻辑 |

依赖关系：`阶段 1（对话）→ 阶段 2（Provider）→ 阶段 3（MCP）→ 阶段 4（设置全切流）→ 阶段 5（桌面/产品）→ 阶段 6（测试质量）→ 阶段 7（Python 退役）`。阶段 5/6 可与 2–4 部分并行。

---

## 阶段 1：核心对话链路 Rust 化（进行中，第一步已完成）

**目标**：Conversation/Session/打断/任务取消/TTS 队列/角色状态/历史全部由 Rust 管理。

- [x] 1.1 SessionActor 扩展：**会话监督状态机已完成**（`rust-gateway/src/session.rs`：Idle/Listening/Generating/Speaking 相位、打断计数、turn 计数、有界 transcript、角色跟踪；`GET /api/v1/session` + `POST /api/v1/session/reset`；消息观察不改变转发语义）
- [x] 1.2 打断语义：**监督层计数与去重已完成**；`cancellation.rs`（CancellationToken 父→子单向传播 + CancellationGuard + wait_for_cancellation）已就绪，待 M2 Provider 接入后落地实际取消传播
- [x] 1.3 会话历史：**内存 transcript 已完成**（`session.rs` 有界 200 条 + 端点）；**持久化（JSON 原子写 + revision）待做**
- [x] 1.4 角色状态：**角色切换捕获 + 快照已完成**；**编排模式已接入**（switch-config → `find_character_prompt` → 重建 system prompt，见 `legacy_settings.rs`）
- [x] 1.5 全链路接管（**含语音闭环**）：`--chat-mode native` 拦截 `text-input`/`ai-speak-signal`/`interrupt-signal`，`conversation.rs` 编排（`ChatSession` + `ActiveTurn` 并发打断 + `run_active_turn` 任务）；**mic-audio-end → `asr-transcribe`（Python 转录 `asr-result` 回 Rust）→ 编排 → `full-text` + `conversation-chain-end` + `tts-speak`（Python 合成 audio payload 播放）**；Python 新增 `_handle_asr_transcribe`/`_handle_tts_speak`（`websocket_handler.py`）；集成测试验证全链路（mock upstream/provider）
- [x] 1.6 旧协议映射：监督层已覆盖 `mic-audio-*`/`audio-*`（V1）双协议与 `text-input`/`interrupt-signal`/`switch-config`/`ai-speak-signal`，转发路径零改动（proxy 模式完全不变）

**验收**
- [x] Rust 集成测试覆盖：会话相位/打断/去重/transcript 上限/角色切换/错误恢复（session 12 项 + cancellation 11 项 + 端点集成 1 项）；编排 7 项；**native 全链路集成 1 项**（text 回合 + mic 回合 → asr-transcribe/tts-speak 往返、mic-audio-end 不泄露）；**Rust 83 项通过**
- [x] 浏览器旧 JSON 音频客户端仍可完整对话（proxy 模式零改动，端到端冒烟验证）
- [x] **默认路径不再调用 Python conversation_handler（native 模式）**：集成测试验证 text/音频回合全程 Rust 编排，Python 仅做 ASR 转录与 TTS 合成
- [x] Python 新增 7 项 native voice handler 单测（转录/空音频/缺引擎/未知客户端/TTS 合成/空文本）；**Python 23 项通过**
- [x] 三平台编译：`cargo test --locked`、Clippy `-D warnings`、`cargo build --release --locked` 全过

---

## 阶段 2：原生 Rust Provider（核心已完成，设置端点待做）

**目标**：对话 LLM 不再依赖 Python；密钥完全由 Rust 持有。

- [x] 2.1 统一 Provider trait（`rust-gateway/src/provider.rs`）：`ChatProvider { stream/complete }`、`ProviderRequest`、`StreamChunk`、`ProviderError`（Auth/RateLimited/Upstream/Network/Timeout/Cancelled/Config/Unsupported）；complete 为 stream 的默认累积实现
- [x] 2.2 OpenAI-compatible 原生实现（reqwest + SSE 流式解析，`data:`/`[DONE]`，tools 增量）
- [x] 2.3 Anthropic Messages API 原生实现（`x-api-key` + `anthropic-version`，SSE content_block 事件，tools 增量）
- [x] 2.4 Ollama 原生实现（`/api/chat` NDJSON，本地 HTTP）
- [x] 2.5 ASR/TTS/本地模型决策矩阵（结论）：**LLM 走 Rust 原生**（OpenAI/Anthropic/Ollama，Ollama 覆盖本地模型）；**ASR/TTS 保留 Python sidecar（可选，默认关闭）**——Rust 生态（whisper-rs/edge-tts 类）质量与维护成本尚不匹配，funasr/faster-whisper/silero 生态成熟；Rust 网关经内部 RPC 调用
- [ ] 2.6 Provider 设置端点 `/api/v1/providers`（列表/能力/健康）+ React UI 切流（仅 `configured` 掩码）——待做

**验收**
- [x] 三个 Provider 单元测试（axum mock HTTP server：文本累积、finish_reason、工具调用增量拼接、401→Auth、429→RateLimited、超时→Timeout、取消→Cancelled、SSE/NDJSON 解析、URL join）——**Rust 75 项全过**
- [x] **不启动 Python LLM 即可完成浏览器对话（Rust Provider 全链路）**：native 模式端到端验证（text-input → 流式回复 → full-text；interrupt 取消）
- [x] 密钥不出 Rust 进程（API key 仅经 env/`ProviderConfig` 注入，无 CLI 参数）；Legacy 适配器仍只回掩码
- [ ] Python stateless LLM 经 RPC 仍可用（回退）——proxy 模式保留，待回归验证

---

## 阶段 3：MCP Rust 化（核心完成）

- [x] 3.1 MCP Server 生命周期：`rust-gateway/src/mcp.rs` stdio 传输（spawn 进程 + newline-delimited JSON-RPC 2.0）、`initialize` 握手、连接超时；`McpRegistry` 多 server 管理；`spawn_and_connect` 便捷构建；CLI `--mcp-server "name=command args"`（可重复/env 分号分隔）
- [x] 3.2 工具注册与参数校验：`tools/list` 加载、`McpToolSpec`、JSON Schema 精简校验器（type/required/properties/items/enum）
- [x] 3.3 调用超时与取消：stdio 读超时（`McpError::Timeout`）+ `CancellationToken` 协作取消（`McpError::Cancelled`）
- [x] 3.4 工具结果注入 Agent 上下文：**Agent 工具循环完成**——`ChatMessage` 扩展（assistant `tool_calls` + `role=tool` 消息，OpenAI/Anthropic 格式）；编排收到 `tool_calls` → 执行（`server.tool`/裸名解析、结果文本提取）→ 回填 → 再调 Provider（`max_tool_rounds` 上限）

**验收**
- [x] MCP 单测 7 项（connect/tools 加载、调用+校验、参数拒绝、未知工具、阻塞请求取消、schema 嵌套/required）+ 工具循环集成测试 1 项（mock provider 两阶段 + InMemory MCP：首轮 tool_call → 执行 → 回填 → 最终文本；验证 `tools/call` 恰一次且参数正确）——**Rust 91 项通过**
- [x] Python `mcpp` 保留可选兼容（proxy 模式不变）
- [x] 三平台：`cargo test --locked`、Clippy `-D warnings`、release 构建全过

---

## 阶段 4：剩余设置切流 + 旧键镜像（Provider/Secret 域完成，UI 面板待做）

- [x] 4.1 **Provider/Secret 设置域（Rust 侧 + React 面板完成）**：`settings.rs` 新增 `provider` 域（kind/base_url/model/api_key）；密钥独立 `secrets.v1.json` 存储（明文仅本机文件，快照只出 `{configured, hint}` 掩码，PATCH `Some(明文)/Some("")/None` 语义）；乐观锁/校验/策略/TS 类型生成齐备；编排从 settings 读 provider 配置；**React Provider 设置面板完成**（新 tab：kind/base_url/model/api_key 三态 `keep/replace/clear`，仅显示 configured+hint，Save/Cancel 事务经 `applyProvider` 走乐观锁，纯函数 `provider-settings-patch.ts` 封装密钥语义）
- [x] 4.2 旧键兼容镜像与 `legacy.extra`（已完成）：`LegacySettingsSnapshotV1.extra` 新增 `unmapped_keys`/`detected_sections`（已知映射键集合 `system_config/character_config/tool_prompts/config_alts_dir`，其余顶层键只读透传+脱敏，供 UI/运维识别未迁移遗留配置）；React 旧键清单（`LEGACY_STORAGE_KEYS`）迁移器保持
- [x] 4.3 双版本回滚窗口（策略已定+测试）：schema 版本校验拒绝未知版本（`unsupported_schema_version` 可操作错误）；v1 读取逻辑保留，v2 引入时保留 v1 读取窗口
- [ ] 4.4 回滚窗口届满后移除旧读取逻辑，Legacy 适配器标记 deprecated 只读——待 Python 退役后统一处理

**验收**
- [x] Provider/Secret 全链路测试（掩码 hint `sk-s…1234`、文件无明文、重载保持、清除/保持语义、URL scheme 校验、策略与 schema catalog 21 字段）；`legacy.extra` 2 项（未映射键探测+脱敏、缺配置时为空）；回滚窗口 1 项（未知 schema 版本拒绝）；**Rust 100 项通过**
- [x] 修复 apply 冲突路径死锁（`self.snapshot()` 重入锁）——settings 冲突测试恢复
- [x] React 类型适配完成（fixture `createProviderPatch`、repository patch 回填 provider 域、validate 用掩码快照、测试断言更新）；**Provider 面板 7 项纯函数测试**（三态密钥语义/trim/URL 归一/dirty 判定）；**Desktop 73 项通过**
- [x] 全链路无明文密钥出 Rust（快照/legacy/UI 均掩码）

---

## 阶段 5：桌面与产品完善（全平台）（进行中）

- [x] 5.1 Rust `/shutdown` HTTP 端点 + supervisor 全平台优雅关闭：watch channel 触发 `with_graceful_shutdown`（Ctrl+C/SIGTERM 并存）；`POST /shutdown` 端到端验证 100ms 内优雅退出；`stopProcess` 渐进式（graceful → POSIX SIGTERM → SIGKILL/taskkill），Python sidecar（无 /shutdown）保留 SIGTERM 优雅路径
- [x] 5.2 替换固定 500ms 模式切换延迟（`window-manager.ts` 删除魔法 sleep，renderer 双 rAF 信号后直接切换）；**显示器增删/分辨率变化事件驱动**（`display-added/removed/metrics-changed` → `refreshPetBounds` 重算跨屏 bounds）
- [x] 5.3 统一鉴权 + HTTP 限流 + 并发限制（已完成）：可选 Bearer token（`OLV_GATEWAY_AUTH_TOKEN` env only，机密不进 argv）保护管理/代理端点（`/api/v1/*`、`/shutdown`、`/metrics`、`/asr`、`/docs` 等），浏览器静态资源与 `/healthz`、`/client-ws` 公开（浏览器 WS 无法带 header，文档注明）；`--http-requests-per-minute-per-ip` 固定窗口每真实对端 IP 限流（从 ConnectInfo extension 读真实 TCP 对端，不信任 X-Forwarded-For）；`--max-concurrent-http` Semaphore 并发上限（503）
- [ ] 5.4 音频协议 V2 评估与实现（分片确认、位深协商等，先出决策记录）——待做
- [ ] 5.5 桌面窗口完善：桌宠模式、托盘、跨屏覆盖、Wayland 适配——待三平台实测

**验收**
- [ ] 三平台行为一致：无魔法延迟；窗口/托盘/跨屏在 Windows/Linux/macOS 实测记录（待 5.5 实测）
- [x] `/shutdown` 集成测试（端点响应 + watch 触发）+ 端到端 100ms 优雅退出；supervisor typecheck 通过
- [x] 鉴权/限流测试：401/200 矩阵、`is_public_path` 分类（资源+WS 公开 vs 管理保护）、限流 429（带 ConnectInfo 的请求）；端到端冒烟：无 token 401、带 token 200、healthz 公开 200、3 次限额后 429；未配置可信代理时只信真实对端（ConnectInfo）

---

## 阶段 6：测试与工程质量（进行中）

- [x] 6.1 React 组件测试（**Provider 设置面板完成**）：引入 jsdom + @testing-library/react + jest-dom（Vitest 复用，未新增框架）；`test-setup.ts` 自动 cleanup；4 项测试（committed 域显示/密钥只显 configured+hint、dirty 提示与 save patch、keep 模式不动密钥、默认模式不清除密钥）
- [ ] 6.2 Web E2E 最小集（Playwright）——待做
- [ ] 6.3 Electron E2E：启动冒烟、设置窗口、桌宠窗口行为——受 harness 限制（Electron GUI 无法启动），需真实平台
- [ ] 6.4 视觉基线（截图对比）——待做
- [x] 6.5 性能/负载测试（**并发会话完成**）：Rust 集成测试 `concurrent_proxy_sessions_serve_all_clients`（12 客户端 × 8 轮 echo 往返经 mock upstream）与 `concurrent_native_sessions_answer_independently`（6 并发客户端各自独立 text-input → full-text）；验证 PeerLimiter/连接 Semaphore 并发拒绝语义
- [ ] 6.6 三平台 GUI 回归矩阵（CI 构建已有，补齐真实启动冒烟）——需真实平台

**验收**
- [ ] 新增模块均有组件/单元测试；桌面 Vitest ≥ 100（当前 77，含 4 项组件测试）
- [ ] CI 三平台绿；Web/Electron E2E 进 CI（Linux 为主，win/mac 可降级为构建+冒烟）（6.2/6.3/6.6）

---

## 阶段 7：Python 退役与收尾（进行中）

- [x] 7.1 默认启动路径完全 Rust（**核心完成**）：Electron `startPython` 改为显式 `OLV_DESKTOP_START_PYTHON=1` 开启（默认关闭）；supervisor 在无 Python 时传 `--allow-missing-python`；Rust gateway 新增 `allow_missing_python`——upstream 连接失败容忍（SessionActor upstream Option 化、转发消息丢弃），native 编排完全自足；**端到端验证：零 Python 进程下 client-ws 连接 + text-input → full-text 完整对话**；集成测试覆盖（无 upstream 对话 + strict 模式拒绝）
- [ ] 7.2 删除 Python 公共服务与默认 Python Runtime（按最终架构决策，保留最小兼容或移除）——待 5.5/6 之后
- [x] 7.3 文档更新（**README 完成**）：架构图（Electron 壳 + Rust 网关 + 可选 Python sidecar）、快速开始（桌面/纯网关/Web 旧链路）、配置表（CLI/env）、安全说明（密钥边界/鉴权/限流）、开发命令、目录与许可证
- [ ] 7.4 全量门禁重跑 + 三平台打包验证
- [ ] 7.5 完成审计（对照本计划逐项核对）→ 提交 goal complete

**验收**
- [x] 无 Python 依赖即可完整启动与对话（端到端：python_procs=0 + full-text 回复；strict 模式无 Python 时正确拒绝）
- [ ] 全门禁绿：Rust/Python(残余)/Desktop 测试、fmt、clippy、ruff、prettier、双构建、`git diff --check`（7.4 最终确认）
- [ ] Windows/Linux/macOS 三平台打包产物可启动（资源、runtime、配置模板、许可证齐全）（7.4）

---

## 风险与阻塞项

| 风险/阻塞 | 影响 | 对策 |
|---|---|---|
| 本 harness 无法启动 Electron GUI（连 `--version` 挂起，环境限制） | 阶段 5/6 GUI 冒烟受阻 | 需用户在本机三平台人工冒烟；CI 承担可自动化部分 |
| 上游 LICENSE 含商业附加条件、Live2D Cubism 专有许可 | 商业分发 | 分发前法律审查（用户决策） |
| WebSDK 586 个既有类型错误（上游债） | 全量 typecheck 红 | 迁移完成后 WebSDK 整体退役即消解；当前只保证新增为 0 |
| 本地模型 Rust 化不确定（whisper-rs 质量/生态） | 阶段 2.5 决策 | 出决策矩阵，ASR/TTS 可长期保留 Python sidecar |
| 本机缺 FFmpeg、可选 Python 依赖未装 | 部分音频 Provider 实跑 | 按需安装（用户环境），不影响默认路径 |
| CI snap/deb 实际打包未验证 | 阶段 7 Linux 分发 | 阶段 7 增加 Linux 安装包构建验证 |

## 里程碑与跟踪

| 里程碑 | 内容 | 状态 |
|---|---|---|
| M0 | 基线固化（清理/网关/设置/桌面壳/打包/CI 三平台/测试基线） | ✅ 完成（Rust 41 / Python 16 / Desktop 66） |
| M1 | 对话链路 Rust 化（阶段 1） | 🔄 进行中（当前缺口最大） |
| M2 | 原生 Rust Provider（阶段 2） | ⬜ |
| M3 | MCP Rust 化（阶段 3） | ✅ 完成（Rust 91 项） |
| M4 | 设置全切流 + 旧键镜像/回滚（阶段 4） | ✅ 完成（Provider/Secret+UI、legacy.extra、回滚策略；4.4 待 Python 退役后执行） |
| M5 | 桌面与产品完善（阶段 5） | 🔄 进行中（5.1/5.2/5.3 完成；5.4 音频 V2、5.5 平台实测待做） |
| M6 | 测试与工程质量（阶段 6） | 🔄 进行中（6.1 组件测试 + 6.5 负载测试完成；6.2/6.3/6.4/6.6 受环境限制待做） |
| M7 | Python 退役 + 完成审计（阶段 7） | 🔄 进行中（7.1 完成——无 Python 完整对话；7.2-7.5 待做） |

**当前进度明细**：Rust 网关 ~95%+、Rust 设置后端 ~98%（M4 完成）、React 设置切流 ~90%（General+Provider 已切流）、Electron 融合 ~85%（5.1/5.2/5.3 完成）、音频前端 ~80%、Conversation/Session/MCP/Provider Rust 化 ~95%（native 编排 + MCP 工具循环 + 无 Python 模式）、测试与工程门禁 ~80%。

**下一步（立即）**：阶段 1 — 从 SessionActor 对话状态机与打断/取消语义开始。
