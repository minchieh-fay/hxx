use std::{collections::HashMap, path::Path, process::Stdio, sync::Arc};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc, mpsc::UnboundedSender},
    time::{Duration, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::audit::AuditLog;
use crate::config::CodexConfig;

/// 表示 HXX 与 Codex App Server 之间可复用的 WebSocket 连接。
type AppServerSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 描述 Codex 单轮对话的文本和 token 用量。
pub struct TurnResult {
    pub text: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 描述从 App Server 收到的一条可展示对话增量或工具活动。
pub struct CodexEvent {
    pub source_id: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub append_detail: bool,
    pub assistant_delta: String,
}

/// 描述浏览器对 App Server 审批请求作出的决定。
struct ApprovalResponse {
    request_id: Value,
    source_id: String,
    decision: String,
}

/// 保存审批请求的界面活动以及批准、拒绝时要回传的协议结果。
struct ApprovalPrompt {
    event: CodexEvent,
    accept_result: Value,
    decline_result: Value,
}

/// 管理 HXX 后端到远程 Codex App Server 的认证连接。
#[derive(Clone)]
pub struct CodexClient {
    url: String,
    token_file: std::path::PathBuf,
    workspace_ssh_target: Option<String>,
    workspace_ssh_root: Option<std::path::PathBuf>,
    workspace_root: Option<std::path::PathBuf>,
    audit: AuditLog,
    active_turns: Arc<Mutex<HashMap<String, String>>>,
    approval_senders: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<ApprovalResponse>>>>,
    thread_sockets: Arc<Mutex<HashMap<String, AppServerSocket>>>,
}

/// 创建连接到远程 App Server 的客户端。
impl CodexClient {
    /// 使用统一配置创建 App Server 客户端和工作区管理器。
    pub fn new(config: CodexConfig, audit: AuditLog) -> Self {
        Self {
            url: config.ws_url,
            token_file: config.token_file,
            workspace_ssh_target: config.workspace_ssh_target,
            workspace_ssh_root: config.workspace_ssh_root,
            workspace_root: config.workspace_root,
            audit,
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            approval_senders: Arc::new(Mutex::new(HashMap::new())),
            thread_sockets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 写入一条协议审计记录，并让日志失败保留在 HXX 服务日志中。
    async fn audit_message(
        &self,
        direction: &str,
        rpc_method: Option<&str>,
        thread_id: Option<&str>,
        message: &Value,
    ) {
        if let Err(error) = self
            .audit
            .record(direction, rpc_method, thread_id, message)
            .await
        {
            eprintln!("HXX 审计日志写入失败: {error:#}");
        }
    }

    /// 生成工作目录，并通过 SSH 或容器共享卷预创建会话目录。
    pub async fn prepare_workspace(&self, client_id: &str, session_id: &str) -> Result<String> {
        // step.1 按浏览器和会话 ID 生成 App Server 容器内的工作路径。
        let cwd = format!("/workspaces/{client_id}/{session_id}");

        // step.2 通过 SSH 或共享卷创建会话目录，供 Codex App Server 使用。
        if let Some(target) = &self.workspace_ssh_target {
            let root = self
                .workspace_ssh_root
                .as_ref()
                .context("设置 SSH 目标时还必须配置 workspace_ssh_root")?;
            let remote_path = format!("{}/{client_id}/{session_id}", root.display());
            let quoted_path = format!("'{}'", remote_path.replace('\'', "'\\''"));
            let command =
                format!("mkdir -p -- {quoted_path} && chown -R 10001:10001 -- {quoted_path}");
            let status = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8"])
                .arg(target)
                .arg(command)
                .status()
                .await
                .context("无法启动 SSH 创建远程工作目录")?;
            if !status.success() {
                return Err(anyhow!("SSH 创建远程 Codex 工作目录失败"));
            }
        } else if let Some(root) = &self.workspace_root {
            let path = root.join(client_id).join(session_id);
            tokio::fs::create_dir_all(&path)
                .await
                .with_context(|| format!("无法创建 Codex 会话工作目录 {}", path.display()))?;
        }
        Ok(cwd)
    }

    /// 将浏览器上传的附件写入指定会话的 Codex 工作目录。
    pub async fn write_workspace_file(
        &self,
        client_id: &str,
        session_id: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<String> {
        // step.1 将受控的用户和会话标识映射到 App Server 工作路径。
        let cwd = format!("/workspaces/{client_id}/{session_id}");
        // step.2 根据共享目录或 SSH 配置将附件字节写入工作目录。
        if let Some(target) = &self.workspace_ssh_target {
            let root = self
                .workspace_ssh_root
                .as_ref()
                .context("设置 SSH 目标时还必须配置 workspace_ssh_root")?;
            let path = root.join(client_id).join(session_id).join(name);
            let quoted_path = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
            let mut child = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8"])
                .arg(target)
                .arg(format!("cat > {quoted_path}"))
                .stdin(Stdio::piped())
                .spawn()
                .context("无法启动 SSH 写入会话附件")?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(bytes).await.context("无法传输会话附件")?;
            }
            let status = child.wait().await.context("等待 SSH 写入附件失败")?;
            if !status.success() {
                return Err(anyhow!("SSH 写入 Codex 会话附件失败"));
            }
        } else if let Some(root) = &self.workspace_root {
            let path = root.join(client_id).join(session_id).join(name);
            tokio::fs::write(&path, bytes)
                .await
                .with_context(|| format!("无法写入会话附件 {}", path.display()))?;
        } else {
            return Err(anyhow!(
                "请配置 workspace_root 或 workspace_ssh_target 才能上传附件"
            ));
        }
        // step.3 返回 App Server 可直接读取的容器内路径。
        Ok(format!("{cwd}/{name}"))
    }

    /// 启动独立 Codex 线程并返回 App Server 分配的线程 ID。
    pub async fn start_thread(&self, cwd: &str) -> Result<String> {
        let params = json!({
            "cwd": cwd,
            "approvalPolicy": "never",
            "sandbox": "danger-full-access"
        });
        let result = self
            .request("thread/start", params, false, None, None)
            .await?;
        result
            .pointer("/thread/id")
            .or_else(|| result.pointer("/id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("Codex App Server 未返回 threadId")
    }

    /// 发送一轮用户消息并等待 Codex 生成结果。
    pub async fn send_turn(
        &self,
        thread_id: &str,
        text: &str,
        image_paths: &[String],
        events: UnboundedSender<CodexEvent>,
    ) -> Result<TurnResult> {
        let mut input = vec![json!({ "type": "text", "text": text })];
        input.extend(image_paths.iter().map(|path| {
            json!({
                "type": "localImage",
                "path": path,
            })
        }));
        let params = json!({
            "threadId": thread_id,
            "input": input,
            "approvalPolicy": "never",
            "sandbox": "danger-full-access"
        });
        let request = self.request("turn/start", params, true, Some(thread_id), Some(&events));
        let response = match timeout(Duration::from_secs(900), request).await {
            Ok(result) => result?,
            Err(_) => {
                self.active_turns.lock().await.remove(thread_id);
                self.approval_senders.lock().await.remove(thread_id);
                return Err(anyhow!("Codex 请求超过 15 分钟仍未完成"));
            }
        };
        // step.1 读取 App Server 返回的当前轮次 token 用量。
        // 调用辅助函数提取本轮输入和输出 token 数。
        let (input_tokens, output_tokens) = crate::help::help_token_counts(&response);

        // step.2 提取流式回复文本，作为本轮持久化的 Codex 消息。
        let mut text = response
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if text.is_empty() {
            text = response
                .get("agentMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
        }
        if text.is_empty() {
            // step.2.1 当流事件未包含最终文本时，从持久化线程历史恢复该轮结果。
            let turn_id = response
                .pointer("/turn/id")
                .or_else(|| response.pointer("/id"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Codex 未返回轮次 ID，无法恢复助手回复"))?;
            let history = self
                .request(
                    "thread/turns/list",
                    json!({ "threadId": thread_id, "limit": 10, "itemsView": "full" }),
                    false,
                    Some(thread_id),
                    None,
                )
                .await?;
            // 调用辅助函数精确读取本轮历史，避免使用其他轮次的消息。
            let turn = crate::help::help_find_turn_history(&history, turn_id)
                .ok_or_else(|| anyhow!("App Server 历史中找不到当前轮次 {turn_id}"))?;
            let status = turn
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if status != "completed" {
                let detail = turn
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("App Server 尚未确认轮次完成");
                return Err(anyhow!("Codex 轮次状态为 {status}: {detail}"));
            }
            // 调用辅助函数提取线程历史中对用户可见的助手消息。
            text = crate::help::help_turn_agent_text(&turn);
            if text.is_empty() {
                return Err(anyhow!("Codex 轮次已完成，但没有助手回复文本"));
            }
            if let Some(items) = turn.get("items").and_then(Value::as_array) {
                for item in items {
                    let notification = json!({ "params": { "item": item } });
                    if let Some(activity) =
                        activity_from_notification(&notification, "item/completed")
                    {
                        let _ = events.send(activity);
                    }
                }
            }
        }
        Ok(TurnResult {
            text,
            input_tokens,
            output_tokens,
        })
    }

    /// 将浏览器对审批卡片的决定转发给当前会话的 App Server 连接。
    pub async fn respond_approval(
        &self,
        thread_id: &str,
        source_id: &str,
        decision: &str,
    ) -> Result<()> {
        // step.1 从活动编号恢复 JSON-RPC 请求 ID 并定位该会话的连接通道。
        let request_id = source_id
            .strip_prefix("approval:")
            .and_then(|value| serde_json::from_str(value).ok())
            .ok_or_else(|| anyhow!("审批请求编号无效"))?;
        let sender = self
            .approval_senders
            .lock()
            .await
            .get(thread_id)
            .cloned()
            .ok_or_else(|| anyhow!("审批连接已关闭，请重新发送消息"))?;
        // step.2 将用户决定排入正在读取 App Server 的异步请求循环。
        sender
            .send(ApprovalResponse {
                request_id,
                source_id: source_id.to_owned(),
                decision: decision.to_owned(),
            })
            .map_err(|_| anyhow!("审批连接已关闭，请重新发送消息"))
    }

    /// 中断当前会话中正在运行的 Codex 轮次。
    pub async fn interrupt(&self, thread_id: &str) -> Result<()> {
        let turn_id = self
            .active_turns
            .lock()
            .await
            .get(thread_id)
            .cloned()
            .ok_or_else(|| anyhow!("该会话当前没有正在运行的请求"))?;
        timeout(
            Duration::from_secs(12),
            self.request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
                false,
                None,
                None,
            ),
        )
        .await
        .map_err(|_| anyhow!("等待 Codex 中断确认超时"))??;
        Ok(())
    }

    /// 将被 HXX 清理的旧会话线程归档到 App Server。
    pub async fn archive_thread(&self, thread_id: &str) -> Result<()> {
        self.request(
            "thread/archive",
            json!({ "threadId": thread_id }),
            false,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    /// 连接 App Server、完成初始化并执行单个 JSON-RPC 请求。
    async fn request(
        &self,
        method: &str,
        params: Value,
        wait_for_turn: bool,
        active_thread: Option<&str>,
        events: Option<&UnboundedSender<CodexEvent>>,
    ) -> Result<Value> {
        // step.1 首轮复用 thread/start 的连接，后续轮次再新建连接并恢复线程订阅。
        let mut socket = if method == "turn/start" {
            let thread_id = active_thread.context("继续 Codex 轮次时缺少 threadId")?;
            self.thread_sockets.lock().await.remove(thread_id)
        } else {
            None
        };
        let started_thread_socket = socket.is_some();
        if socket.is_none() {
            let token = read_token(&self.token_file).await?;
            let mut request = self.url.as_str().into_client_request()?;
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {token}")
                    .parse()
                    .context("Codex token 格式无效")?,
            );
            let (connected_socket, _) = timeout(Duration::from_secs(12), connect_async(request))
                .await
                .map_err(|_| anyhow!("连接 Codex App Server 超时"))?
                .context("无法连接 Codex App Server")?;
            socket = Some(connected_socket);
        }
        let mut socket = socket.context("Codex App Server WebSocket 未建立")?;

        // step.2 新连接完成初始化握手；复用 thread/start 的连接时无需重复握手。
        if !started_thread_socket {
            let initialize = json!({
                "id": 0,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "hxx",
                        "title": "HXX",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": { "experimentalApi": true }
                }
            });
            self.audit_message("outgoing", Some("initialize"), active_thread, &initialize)
                .await;
            socket
                .send(Message::Text(initialize.to_string().into()))
                .await?;
            timeout(
                Duration::from_secs(12),
                wait_for_response(
                    &mut socket,
                    0,
                    &self.audit,
                    Some("initialize"),
                    active_thread,
                ),
            )
            .await
            .map_err(|_| anyhow!("等待 Codex App Server 初始化超时"))??;
            let initialized = json!({ "method": "initialized", "params": {} });
            self.audit_message("outgoing", Some("initialized"), active_thread, &initialized)
                .await;
            socket
                .send(Message::Text(initialized.to_string().into()))
                .await?;
        }

        // step.3 新连接恢复已有线程；新线程首轮沿用已订阅线程事件的创建连接。
        let request_id = if method == "turn/start" && started_thread_socket {
            1
        } else if method == "turn/start" {
            let thread_id = active_thread.context("继续 Codex 轮次时缺少 threadId")?;
            let resume = json!({
                "id": 1,
                "method": "thread/resume",
                "params": { "threadId": thread_id }
            });
            self.audit_message("outgoing", Some("thread/resume"), Some(thread_id), &resume)
                .await;
            socket
                .send(Message::Text(resume.to_string().into()))
                .await?;
            let resume_result = wait_for_response(
                &mut socket,
                1,
                &self.audit,
                Some("thread/resume"),
                Some(thread_id),
            )
            .await;
            match resume_result {
                Ok(_) => 2,
                Err(error) if error.to_string().contains("no rollout found for thread id") => {
                    eprintln!("HXX 新线程尚无历史记录，首轮直接启动 thread_id={thread_id}");
                    1
                }
                Err(error) => return Err(error),
            }
        } else {
            1
        };

        // step.4 登记当前轮次的审批回传通道，并发送 Codex 业务请求。
        let (approval_sender, mut approval_receiver) = mpsc::unbounded_channel();
        let approval_channel_active = wait_for_turn && active_thread.is_some();
        if approval_channel_active {
            if let Some(thread_id) = active_thread {
                self.approval_senders
                    .lock()
                    .await
                    .insert(thread_id.to_owned(), approval_sender);
            }
        }
        let outgoing = json!({ "id": request_id, "method": method, "params": params });
        self.audit_message("outgoing", Some(method), active_thread, &outgoing)
            .await;
        socket
            .send(Message::Text(outgoing.to_string().into()))
            .await?;
        let mut result = Value::Null;
        let mut output = String::new();
        let mut input_tokens = 0;
        let mut output_tokens = 0;
        let mut response_received = false;
        let mut turn_finished = !wait_for_turn;
        let mut turn_failed = false;
        let mut approval_results: HashMap<String, (Value, Value)> = HashMap::new();
        loop {
            let message = tokio::select! {
                message = socket.next() => message.transpose()?,
                approval = approval_receiver.recv(), if approval_channel_active => {
                    if let Some(approval) = approval {
                        let accepted = approval.decision == "accept";
                        let result = approval_results
                            .remove(&approval.source_id)
                            .map(|(accept_result, decline_result)| {
                                if accepted {
                                    accept_result
                                } else {
                                    decline_result
                                }
                            })
                            .unwrap_or_else(|| json!({ "decision": approval.decision.clone() }));
                        let outgoing = json!({ "id": approval.request_id, "result": result });
                        self.audit_message(
                            "outgoing",
                            Some("server-request-response"),
                            active_thread,
                            &outgoing,
                        )
                        .await;
                        socket
                            .send(Message::Text(outgoing.to_string().into()))
                            .await?;
                        if let Some(sender) = events {
                            let _ = sender.send(CodexEvent {
                                source_id: approval.source_id,
                                kind: "approval".to_owned(),
                                title: if accepted { "已批准" } else { "已拒绝" }.to_owned(),
                                detail: String::new(),
                                status: if accepted { "completed" } else { "interrupted" }
                                    .to_owned(),
                                append_detail: false,
                                assistant_delta: String::new(),
                            });
                        }
                    }
                    continue;
                }
            };
            let Some(message) = message else {
                break;
            };
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            self.audit_message("incoming", Some(method), active_thread, &value)
                .await;
            if let Some(approval) = approval_activity_from_request(&value) {
                approval_results.insert(
                    approval.event.source_id.clone(),
                    (approval.accept_result, approval.decline_result),
                );
                if let Some(sender) = events {
                    let _ = sender.send(approval.event);
                }
                continue;
            }
            if value.get("id").and_then(Value::as_u64) == Some(request_id) {
                if let Some(error) = value.get("error") {
                    return Err(anyhow!("Codex 请求失败: {error}"));
                }
                result = value.get("result").cloned().unwrap_or(Value::Null);
                response_received = true;
                eprintln!("HXX App Server 已响应 method={method}");
                if let Some(sender) = events {
                    let _ = sender.send(CodexEvent {
                        source_id: "hxx:connecting".to_owned(),
                        kind: "connection".to_owned(),
                        title: "已连接 Codex App Server".to_owned(),
                        detail: String::new(),
                        status: "completed".to_owned(),
                        append_detail: false,
                        assistant_delta: String::new(),
                    });
                }
                if let Some(thread_id) = active_thread {
                    if let Some(turn_id) = result
                        .pointer("/turn/id")
                        .or_else(|| result.pointer("/id"))
                        .and_then(Value::as_str)
                    {
                        self.active_turns
                            .lock()
                            .await
                            .insert(thread_id.to_owned(), turn_id.to_owned());
                    }
                }
            }
            let event = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let legacy_type = value
                .pointer("/params/msg/type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !event.is_empty() {
                eprintln!("HXX App Server 事件 method={event}");
            }
            if event == "turn/started" {
                if let (Some(thread_id), Some(turn_id)) = (
                    active_thread,
                    value.pointer("/params/turn/id").and_then(Value::as_str),
                ) {
                    self.active_turns
                        .lock()
                        .await
                        .insert(thread_id.to_owned(), turn_id.to_owned());
                }
            }
            if event == "thread/status/changed"
                && value.pointer("/params/threadId").and_then(Value::as_str) == active_thread
            {
                if let Some(activity) = thread_status_activity(&value) {
                    if let Some(sender) = events {
                        let _ = sender.send(activity);
                    }
                }
                let status = value.pointer("/params/status/type").and_then(Value::as_str);
                if response_received && status == Some("idle") {
                    turn_finished = true;
                }
            }
            if event == "turn/plan/updated" {
                if let Some(activity) = plan_activity(&value) {
                    if let Some(sender) = events {
                        let _ = sender.send(activity);
                    }
                }
            }
            let is_agent_delta = event == "item/agentMessage/delta"
                || (event == "codex/event" && legacy_type == "AgentMessageDelta");
            if is_agent_delta {
                let delta = value
                    .pointer("/params/delta")
                    .or_else(|| value.pointer("/params/msg/delta"))
                    .and_then(Value::as_str);
                if let Some(delta) = delta {
                    output.push_str(delta);
                    if let Some(sender) = events {
                        let _ = sender.send(CodexEvent {
                            source_id: String::new(),
                            kind: "assistant".to_owned(),
                            title: String::new(),
                            detail: String::new(),
                            status: "running".to_owned(),
                            append_detail: false,
                            assistant_delta: delta.to_owned(),
                        });
                    }
                }
            }
            if let Some(activity) = activity_from_notification(&value, event) {
                if let Some(sender) = events {
                    let _ = sender.send(activity);
                }
            }
            if event == "thread/tokenUsage/updated" {
                // step.3.1 汇总本轮流式通知中的 token 统计。
                let counts = crate::help::help_token_counts(&value);
                input_tokens = counts.0;
                output_tokens = counts.1;
            }
            if event == "turn/completed"
                || (event == "codex/event"
                    && matches!(legacy_type, "TurnComplete" | "TurnCompleted"))
            {
                turn_finished = true;
                let status = value
                    .pointer("/params/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                if status == "failed" {
                    turn_failed = true;
                }
            }
            if response_received && turn_finished {
                break;
            }
        }
        if let Some(thread_id) = active_thread {
            self.active_turns.lock().await.remove(thread_id);
            self.approval_senders.lock().await.remove(thread_id);
        }
        if wait_for_turn && !turn_finished {
            return Err(anyhow!("Codex App Server 在本轮完成前关闭连接"));
        }
        if turn_failed {
            return Err(anyhow!("Codex 本轮请求失败"));
        }
        if result.get("error").is_some() {
            return Err(anyhow!("Codex 返回错误: {}", result["error"]));
        }
        if !output.is_empty() {
            result["text"] = Value::String(output);
        }
        if input_tokens > 0 || output_tokens > 0 {
            result["tokenUsage"] = json!({
                "last": {
                    "inputTokens": input_tokens,
                    "outputTokens": output_tokens
                }
            });
        }
        if method == "thread/start" {
            if let Some(thread_id) = result
                .pointer("/thread/id")
                .or_else(|| result.pointer("/id"))
                .and_then(Value::as_str)
            {
                self.thread_sockets
                    .lock()
                    .await
                    .insert(thread_id.to_owned(), socket);
            }
        }
        Ok(result)
    }
}

/// 将 App Server 通知转换为对话框可展示的工具活动。
fn activity_from_notification(value: &Value, event: &str) -> Option<CodexEvent> {
    if event == "codex/event" {
        return legacy_activity(value);
    }

    // step.1 将轮次开始和完成通知转成可见的处理状态。
    if event == "turn/started" || event == "turn/completed" {
        let turn_id = value
            .pointer("/params/turn/id")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let raw_status = value
            .pointer("/params/turn/status")
            .and_then(Value::as_str)
            .unwrap_or(if event == "turn/started" {
                "inProgress"
            } else {
                "completed"
            });
        let status = match raw_status {
            "inProgress" | "running" => "running",
            other => other,
        };
        let title = match status {
            "completed" => "本轮完成",
            "interrupted" => "本轮已中断",
            "failed" => "本轮执行失败",
            _ if event == "turn/started" => "Codex 开始处理",
            _ => "Codex 处理中",
        };
        return Some(CodexEvent {
            source_id: format!("turn:{turn_id}"),
            kind: "turn".to_owned(),
            title: title.to_owned(),
            detail: String::new(),
            status: status.to_owned(),
            append_detail: false,
            assistant_delta: String::new(),
        });
    }

    // step.2 将命令输出、文件修改和外部工具调用转成活动记录。
    if event == "item/commandExecution/outputDelta" {
        let params = value.get("params")?;
        let item_id = params.get("itemId")?.as_str()?;
        return Some(CodexEvent {
            source_id: format!("item:{item_id}"),
            kind: "commandExecution".to_owned(),
            title: "运行命令".to_owned(),
            detail: params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            status: "running".to_owned(),
            append_detail: true,
            assistant_delta: String::new(),
        });
    }
    if event != "item/started" && event != "item/completed" {
        return None;
    }
    let item = value.pointer("/params/item")?;
    let kind = item.get("type")?.as_str()?;
    let item_id = item.get("id")?.as_str()?;
    let (title, detail) = match kind {
        "commandExecution" => (
            if event == "item/started" {
                "运行命令"
            } else {
                "命令执行完成"
            },
            if event == "item/started" {
                item.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            } else {
                item.get("aggregatedOutput")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            },
        ),
        "fileChange" => (
            if event == "item/started" {
                "修改文件"
            } else {
                "文件修改完成"
            },
            item.get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.get("path").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        ),
        "mcpToolCall" => (
            "调用工具",
            format!(
                "{}/{}",
                item.get("serverName")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp"),
                item.get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
            ),
        ),
        "webSearch" => (
            "搜索网页",
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        "imageGeneration" => ("生成图像", String::new()),
        "collabAgentToolCall" => ("委派子任务", String::new()),
        _ => return None,
    };
    let raw_status =
        item.get("status")
            .and_then(Value::as_str)
            .unwrap_or(if event == "item/started" {
                "running"
            } else {
                "completed"
            });
    let status = match raw_status {
        "completed" | "succeeded" => "completed",
        "failed" => "failed",
        "declined" | "interrupted" => "interrupted",
        _ => "running",
    };
    Some(CodexEvent {
        source_id: format!("item:{item_id}"),
        kind: kind.to_owned(),
        title: title.to_owned(),
        detail,
        status: status.to_owned(),
        append_detail: false,
        assistant_delta: String::new(),
    })
}

/// 将 App Server 发起的命令、文件或权限审批请求转换为可交互活动。
fn approval_activity_from_request(value: &Value) -> Option<ApprovalPrompt> {
    // step.1 识别 App Server 审批类型并提取可供用户判断的信息。
    let method = value.get("method")?.as_str()?;
    let request_id = value.get("id")?;
    let params = value.get("params")?;
    let (title, detail, accept_result, decline_result) = match method {
        "item/commandExecution/requestApproval" => {
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("未提供命令详情");
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let detail = [
                format!("$ {command}"),
                if cwd.is_empty() {
                    String::new()
                } else {
                    format!("工作目录：{cwd}")
                },
                if reason.is_empty() {
                    String::new()
                } else {
                    format!("原因：{reason}")
                },
            ]
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
            (
                "请求执行命令",
                detail,
                json!({ "decision": "accept" }),
                json!({ "decision": "decline" }),
            )
        }
        "item/fileChange/requestApproval" => {
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Codex 请求批准文件变更");
            (
                "请求批准文件变更",
                reason.to_owned(),
                json!({ "decision": "accept" }),
                json!({ "decision": "decline" }),
            )
        }
        "item/permissions/requestApproval" => {
            let permissions = params.get("permissions")?.clone();
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Codex 请求额外权限");
            let detail = format!(
                "{reason}\n请求权限：{}",
                serde_json::to_string_pretty(&permissions).ok()?
            );
            (
                "请求额外权限",
                detail,
                json!({ "permissions": permissions, "scope": "session" }),
                json!({ "permissions": {}, "scope": "turn" }),
            )
        }
        _ => return None,
    };
    // step.2 保留 JSON-RPC 请求 ID，供浏览器决定回传到原连接。
    Some(ApprovalPrompt {
        event: CodexEvent {
            source_id: format!("approval:{}", request_id),
            kind: "approval".to_owned(),
            title: title.to_owned(),
            detail,
            status: "running".to_owned(),
            append_detail: false,
            assistant_delta: String::new(),
        },
        accept_result,
        decline_result,
    })
}

/// 将 Codex 的计划更新转换为对话框中的任务步骤和完成状态。
fn plan_activity(value: &Value) -> Option<CodexEvent> {
    // step.1 提取计划步骤并整理每一步的当前状态。
    let turn_id = value.pointer("/params/turnId")?.as_str()?;
    let steps = value.pointer("/params/plan")?.as_array()?;
    let detail = steps
        .iter()
        .filter_map(|step| {
            let title = step.get("step")?.as_str()?;
            let status = match step.get("status")?.as_str()? {
                "completed" => "已完成",
                "inProgress" => "进行中",
                _ => "待处理",
            };
            Some(format!("[{status}] {title}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    // step.2 根据未完成步骤标记计划仍在执行或已经结束。
    let is_running = steps.iter().any(|step| {
        step.get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "completed")
    });
    Some(CodexEvent {
        source_id: format!("plan:{turn_id}"),
        kind: "plan".to_owned(),
        title: "执行计划".to_owned(),
        detail,
        status: if is_running { "running" } else { "completed" }.to_owned(),
        append_detail: false,
        assistant_delta: String::new(),
    })
}

/// 将线程运行状态和等待原因转换成对话框中的可见活动。
fn thread_status_activity(value: &Value) -> Option<CodexEvent> {
    let thread_id = value.pointer("/params/threadId")?.as_str()?;
    let status = value.pointer("/params/status/type")?.as_str()?;
    let flags = value
        .pointer("/params/status/activeFlags")
        .and_then(Value::as_array);
    let waiting = flags.and_then(|flags| {
        flags.iter().find_map(|flag| match flag.as_str()? {
            "waitingOnApproval" => Some("等待批准"),
            "waitingOnUserInput" => Some("等待补充输入"),
            _ => None,
        })
    });
    let (title, activity_status) = match (status, waiting) {
        (_, Some(title)) => (title, "running"),
        ("active", None) => ("Codex 正在运行", "running"),
        ("idle", None) => ("Codex 当前空闲", "completed"),
        ("systemError", None) => ("Codex 遇到系统错误", "failed"),
        ("notLoaded", None) => ("Codex 线程未加载", "completed"),
        _ => return None,
    };
    Some(CodexEvent {
        source_id: format!("thread-status:{thread_id}"),
        kind: "threadStatus".to_owned(),
        title: title.to_owned(),
        detail: String::new(),
        status: activity_status.to_owned(),
        append_detail: false,
        assistant_delta: String::new(),
    })
}

/// 将旧版 App Server 命令事件转换成统一工具活动记录。
fn legacy_activity(value: &Value) -> Option<CodexEvent> {
    // step.1 识别旧协议中的命令开始、输出和完成事件。
    let message = value.pointer("/params/msg")?;
    let event_type = message.get("type")?.as_str()?;
    let call_id = message
        .get("call_id")
        .or_else(|| message.get("callId"))
        .and_then(Value::as_str)
        .unwrap_or("command");
    let is_output = event_type == "ExecCommandOutputDelta";
    let is_start = event_type == "ExecCommandBegin";
    let is_end = event_type == "ExecCommandEnd";
    if !is_output && !is_start && !is_end {
        return None;
    }

    // step.2 整理命令文本和逐步输出，并映射命令运行状态。
    let detail = if is_output {
        message
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    } else if is_start {
        message
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    } else {
        String::new()
    };
    Some(CodexEvent {
        source_id: format!("item:{call_id}"),
        kind: "commandExecution".to_owned(),
        title: if is_end {
            "命令执行完成".to_owned()
        } else {
            "运行命令".to_owned()
        },
        detail,
        status: if is_end {
            "completed".to_owned()
        } else {
            "running".to_owned()
        },
        append_detail: is_output,
        assistant_delta: String::new(),
    })
}

/// 从统一配置指定的本地文件读取 App Server capability token。
async fn read_token(path: &Path) -> Result<String> {
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("无法读取 Codex App Server token 文件 {}", path.display()))
        .map(|token| token.trim().to_owned())
}

/// 等待指定 JSON-RPC 请求的响应并解析服务端错误。
async fn wait_for_response<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    request_id: u64,
    audit: &AuditLog,
    rpc_method: Option<&str>,
    thread_id: Option<&str>,
) -> Result<Value>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // step.1 忽略服务端通知，直到匹配初始化请求 ID 的响应到达。
    while let Some(message) = socket.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text)?;
        if let Err(error) = audit
            .record("incoming", rpc_method, thread_id, &value)
            .await
        {
            eprintln!("HXX 审计日志写入失败: {error:#}");
        }
        if value.get("id").and_then(Value::as_u64) != Some(request_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(anyhow!("Codex 初始化失败: {error}"));
        }

        // step.2 返回初始化结果供后续 JSON-RPC 请求继续使用。
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    Err(anyhow!("Codex App Server 在初始化时关闭连接"))
}
