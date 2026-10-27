# 项目协作规则

开始实现功能前，先阅读 [`docs/architecture.md`](docs/architecture.md)，了解项目边界、Codex App Server 接入方式和隔离要求。该文档是项目设计说明，应该纳入 Git。

执行本地打包或部署任务时，如果存在 `docs/AGENTS.local.md`，可读取其中的本机流程。该文件含个人环境信息，必须保持在 `.gitignore` 排除范围内，不得提交或复制到公开文档。

## 编码规则

### 函数和类型注释

- 新建函数、类、结构体或等价类型时，必须在定义上方用中文注释说明用途。
- 每个 `help_*` 调用点都必须有中文注释，说明该调用在当前业务中的作用。
- 函数超过 20 行时，必须在函数内部用中文 `step` 注释概括业务逻辑。
- `step` 按业务目标分层，不按 `if`、循环等控制流划分。读完 `step` 注释后，应能理解函数的完整业务逻辑。

### 辅助函数

- 所有 `help_*` 辅助函数都必须定义在所属业务代码目录中的语言专属 `help` 文件里，例如 `help.rs`、`help.ts`。
- `help` 文件必须与调用方处于同一目录，且只能由该目录内代码调用。
- `help` 函数上方也必须有中文用途注释；超过 20 行时添加符合业务层级的 `step` 注释。
- 主业务函数包含多个业务步骤时，应将可独立提取的多行实现放入同目录的 `help` 文件。

### TypeScript

- 单行代码不超过 120 个字符。
- 遵循项目格式化工具和现有代码风格，避免难以阅读的长行。

## 本地配置与凭据

- 不要提交 API token、App Server capability token、个人运行状态、下载的可执行文件或镜像归档。
- 私有代理、内部网关地址和个人打包环境只保存在本地忽略文件中；示例配置必须使用占位值。


## 开发阶段
我们不用编译hxx到服务器, 就本地调试  连接服务器的4500端口(codex 的appserver)

## 发布部署

- 开发调试时 HXX 仍在 macOS 本机运行；只有明确进行部署时，才将构建产物发布到开发服务器。
- 前端在本机运行 `cd frontend && npm ci && npm run build`，部署 `frontend/dist/`。
- Rust 后端在 macOS 本机使用 `cargo-zigbuild` 和 Zig 交叉编译为 Linux amd64，目标 glibc 与 `codex-web:latest` 的 Debian Bookworm 运行环境兼容：
  `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.36`。
- 不在开发服务器编译 HXX，不使用 Dockerfile 构建或发布 HXX 镜像。只将后端可执行文件和前端 `dist/` 传到本机部署说明指定的 HXX 目录。
- HXX 服务复用 `codex-web:latest` 作为运行环境。Compose 必须同时覆盖镜像默认的 Codex `entrypoint` 和 `command`，以 `/usr/bin/tini -- /app/hxx-web` 启动 HXX；工作目录设为 `/app`，前端静态文件位于 `/app/frontend/dist`。
- HXX 的服务、Codex、token 文件路径、工作区和审计设置统一保存在 `hxx.toml`；部署时挂载该文件，不为 HXX 另建 `.env` 或 Compose `env_file`。
- 仓库根目录的 `docker-compose.yaml` 是 HXX 运行配置，部署时复制到服务器 HXX 目录作为服务 Compose 文件。
- 部署目录中的二进制和 `frontend/dist/` 以只读卷挂载；SQLite、审计日志等运行数据使用独立可写目录。容器以镜像默认 UID `10001` 运行，部署前确保数据目录归该 UID 写入。
- HXX 与 Codex App Server 加入已有外部 `app-net` 网络，HXX 服务使用 `hxx.internal` 别名并通过 `ws://codex.internal:4500` 连接；capability token 从只读挂载的秘密文件读取，不写入 Compose 样例、命令行或 Git。
- 传输时只更新后端二进制和前端构建产物。若需修改服务器上的 `docker-compose.yaml`，只合并本次变更字段，保留服务器的本地配置；不要用仓库样例覆盖整份文件。变更后在服务器运行 `docker-compose config` 检查，再按本机部署说明启动或重建 HXX 服务。
- 服务器地址、部署绝对路径和个人凭据只记录在被 `.gitignore` 排除的 `docs/AGENTS.local.md`，不得复制到本文件或提交到 Git。
