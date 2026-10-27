# HXX 设计说明

## 目标

- 为团队提供基于 Codex App Server 的 Web 工作台。
- 每个用户可创建多个会话，并为不同任务使用独立工作目录。
- 用户可显式整理并共享经过确认的团队经验，不自动共享个人聊天记录。
- Codex CLI 与 App Server 由运维人员按版本更新。

## 目录边界

- 仓库根目录是 HXX Web 应用，包含 Rust 后端、Vue 前端和通用构建配置。
- `codex/` 管理独立运行的 Codex App Server 镜像、部署模板和配置模板。
- App Server 二进制、helper、运行状态、token、个人配置和镜像归档只保存在本地忽略路径。
- 仓库中的配置样例不得包含私有 API 地址、代理地址、凭据或个人打包环境。

## 请求架构

```text
Browser
  │ HXX authentication and session authorization
  ▼
Rust web backend
  ├── user / session / thread / workspace mapping
  ├── creates and validates workspace paths
  └── JSON-RPC WebSocket client
          │ server-side capability token
          ▼
Codex App Server
  ├── CODEX_HOME: user or instance state
  └── cwd: session workspace
```

浏览器只连接 HXX 后端。后端负责用户身份、会话权限、线程归属、工作目录分配和事件流；App Server 的 capability token 与模型 API token 不得发送给浏览器。

## App Server 生命周期

- 创建会话时调用 `thread/start`，为线程分配工作目录并保存 `threadId`；首轮复用创建连接启动，持续接收线程事件。
- 恢复会话时调用 `thread/resume`，并通过线程历史接口重建 UI。
- `CODEX_HOME` 是 App Server 进程级状态目录；`cwd` 是会话或任务的工作目录，两者职责不同。
- 浏览器断线后重新连接并从持久化线程历史恢复界面，不假定断线期间的每个流式事件都会自动重放。
- 当前 Codex 部署使用 YOLO 模式；HXX 创建线程和轮次时显式设置 `approvalPolicy=never` 与 `sandbox=danger-full-access`。
- App Server 如果仍发起命令、文件变更或额外权限审批，HXX 会在会话中显示请求详情并提供批准和拒绝操作。
- HXX 从 `hxx.toml` 读取运行配置；协议审计按日追加到 JSONL，正文采集通过 `audit.include_payloads` 单独控制。
- `codex-code-mode-host` 如启用，应与 Codex CLI 使用同一 release 和目标平台。

## HXX 首版行为

- HXX 开发期间在开发机本地运行 Rust API 和 Vue 页面，通过 `CODEX_WS_URL` 连接开发服务器上的 App Server。
- 浏览器在 `localStorage` 中保存稳定 ID，用于区分用户和隔离会话；该 ID 不提供身份认证。
- 用户可设置显示名称，后端记录最近访问 IP 供内网排查问题。
- 每个浏览器最多保留十个会话；创建新会话时删除最早的会话并归档对应 Codex 线程，历史用量记录继续保留。
- Codex 工作目录按 `/workspaces/{client_id}/{session_id}` 分开，对应 App Server 的 `workspace/` 挂载目录。
- 助手文本增量、执行计划和命令/工具活动写入会话，前端以短轮询展示执行状态和秒级耗时，不暴露内部推理内容。
- SQLite 保存用户、会话、消息和每轮请求 token 用量；仪表盘按本机时区汇总当天、本周和本月数据。
- 当实时轮次事件缺少最终消息时，HXX 查询 App Server 完整线程历史恢复助手文本和工具活动；未确认完成的轮次会作为错误显示，不保存为空回复。

## Web 技术栈

- 后端：Rust、Axum、Tokio。第一版可由 Rust 服务托管 Vue 构建产物。
- 前端：Vue 3、TypeScript、Vite。浏览器通过 HXX 后端接收流式事件和发送操作。
- 初期可用 SQLite 保存用户、线程、工作目录及 UI 恢复所需状态；有多实例或更高并发需求后再迁移 PostgreSQL。
- App Server 使用 JSON-RPC over WebSocket。Rust 可先用 `serde_json::Value` 处理协议，再为实际使用的请求和事件定义类型。
- 固定 Codex CLI 版本，并在升级时比较该版本生成的 JSON Schema。

## 隔离与安全

- 单个 App Server 和共享 `CODEX_HOME` 适用于可信团队原型，不构成用户之间的强隔离。
- 启用 approval 和 sandbox bypass 后，Codex 可能访问进程可见的全部挂载内容。多人长期使用前，应按用户隔离 App Server 进程、`CODEX_HOME` 和挂载工作区；每个用户的会话再分配独立 `cwd`。
- 后端必须验证每个 `threadId` 和 `cwd` 的用户归属，不信任浏览器传入的路径或线程 ID。
- 不要把 Docker socket 挂载到 Web 应用容器。
- 团队知识应单独维护并经过确认，再按需加入相关会话或工作区。

## 部署说明

- `codex/` 中带 `.example` 的文件是可提交模板；本地实际配置和凭据位于 `.gitignore` 明确排除的路径。
- App Server WebSocket transport 当前为实验性功能。只在可信服务网络中开放，并启用 capability-token 认证；不要直接发布到公网。
- Web 后端负责用户鉴权和会话授权，App Server 的连接认证不能替代应用层权限控制。

## 官方资料

- [Codex App Server](https://developers.openai.com/codex/app-server/)
- [Codex configuration reference](https://developers.openai.com/codex/config-reference/)
- [OpenAI Codex source repository](https://github.com/openai/codex)
