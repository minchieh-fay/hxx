use std::net::SocketAddr;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    codex::{CodexClient, CodexEvent},
    db::{Activity, Database},
};

/// 描述处理 HXX API 请求所需的共享服务。
#[derive(Clone)]
pub struct AppState {
    pub database: std::sync::Arc<Database>,
    pub codex: CodexClient,
}

/// 描述本地 HXX 服务对外提供的 API 路由。
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/profile", get(get_profile).patch(update_profile))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{session_id}", get(get_session))
        .route("/api/sessions/{session_id}/turns", post(send_turn))
        .route("/api/sessions/{session_id}/interrupt", post(interrupt))
        .route(
            "/api/sessions/{session_id}/approvals",
            post(respond_approval),
        )
        .route("/api/dashboard", get(dashboard))
        .with_state(state)
}

/// 描述 API 处理失败时返回的状态码和中文错误信息。
struct ApiError(StatusCode, String);

/// 将业务错误转换为统一 JSON HTTP 响应。
impl IntoResponse for ApiError {
    /// 把内部状态码和错误文本编码成 JSON 响应体。
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

/// 描述浏览器创建新会话时提交的内容。
#[derive(Deserialize)]
pub struct NewSession {
    topic: String,
    environment: String,
    issue: String,
    work: String,
    notes: String,
}

/// 描述浏览器向现有会话发送的消息。
#[derive(Deserialize)]
pub struct NewTurn {
    text: String,
}

/// 描述用户对 Codex 工具审批请求的决定。
#[derive(Deserialize)]
struct ApprovalDecision {
    source_id: String,
    decision: String,
}

/// 描述用户修改个人显示名称的请求。
#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

/// 描述会话页面需要的会话元数据和完整消息列表。
#[derive(Serialize)]
struct SessionResponse {
    session: crate::db::Session,
    messages: Vec<crate::db::Message>,
    activities: Vec<Activity>,
}

/// 返回 HXX API 的健康状态。
async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// 返回当前浏览器身份对应的用户资料和最近访问 IP。
async fn get_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Result<Json<crate::db::Profile>, ApiError> {
    // step.1 校验名称长度并限制单个浏览器身份的资料更新。
    let client_id = client_id(&headers)?;
    let name = headers
        .get("x-client-name")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(&client_id);
    let profile = state
        .database
        .touch_profile(&client_id, name, &address.ip().to_string())
        .map_err(internal_error)?;
    Ok(Json(profile))
}

/// 保存用户修改后的显示名称并返回更新资料。
async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<RenameRequest>,
) -> Result<Json<crate::db::Profile>, ApiError> {
    let client_id = client_id(&headers)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "名字长度必须在 1 到 40 个字符之间".to_owned(),
        ));
    }

    // step.2 保存新名称和当前访问 IP 并返回更新结果。
    let profile = state
        .database
        .rename_profile(&client_id, name, &address.ip().to_string())
        .map_err(internal_error)?;
    Ok(Json(profile))
}

/// 返回当前浏览器身份拥有的会话列表。
async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Result<Json<Vec<crate::db::Session>>, ApiError> {
    let client_id = client_id(&headers)?;
    state
        .database
        .touch_profile(&client_id, &client_id, &address.ip().to_string())
        .map_err(internal_error)?;
    let sessions = state
        .database
        .list_sessions(&client_id)
        .map_err(internal_error)?;
    Ok(Json(sessions))
}

/// 创建会话、启动远程线程并发送确认后的 Markdown 首条消息。
async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<NewSession>,
) -> Result<Json<SessionResponse>, ApiError> {
    // step.1 校验浏览器身份和会话说明并确保用户记录存在。
    let client_id = client_id(&headers)?;
    validate_session(&request)?;
    state
        .database
        .touch_profile(&client_id, &client_id, &address.ip().to_string())
        .map_err(internal_error)?;
    let session_id = Uuid::new_v4().to_string();
    let cwd = state
        .codex
        .prepare_workspace(&client_id, &session_id)
        .await
        .map_err(codex_error)?;

    // step.2 在远程 App Server 创建该用户和会话专属的工作线程。
    let thread_id = state.codex.start_thread(&cwd).await.map_err(codex_error)?;
    let title = request.topic.trim();
    let title = if title.is_empty() { "新会话" } else { title };
    let evicted = state
        .database
        .create_session(&client_id, &session_id, &thread_id, title, &cwd)
        .map_err(internal_error)?;
    if let Some(evicted) = evicted {
        let codex = state.codex.clone();
        tokio::spawn(async move {
            let _ = codex.archive_thread(&evicted.thread_id).await;
        });
    }

    // step.3 保存说明并启动会话首轮 Codex 对话。
    // 调用辅助函数将会话表单整理成完整 Markdown 首条消息。
    let prompt = crate::help::help_build_session_prompt(
        &request.topic,
        &request.environment,
        &request.issue,
        &request.work,
        &request.notes,
    );
    if !state
        .database
        .begin_turn(&client_id, &session_id, &prompt)
        .map_err(internal_error)?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "会话当前正在处理其他请求".to_owned(),
        ));
    }
    spawn_turn(
        state.clone(),
        client_id.clone(),
        session_id.clone(),
        thread_id,
        prompt,
    );
    session_response(&state.database, &client_id, &session_id)
        .map(Json)
        .map_err(internal_error)
}

/// 返回会话元数据和当前浏览器身份可访问的对话记录。
async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<SessionResponse>, ApiError> {
    let client_id = client_id(&headers)?;
    session_response(&state.database, &client_id, &session_id)
        .map(Json)
        .map_err(internal_error)
}

/// 保存用户消息并在后台处理 Codex 回复和用量。
async fn send_turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<NewTurn>,
) -> Result<Json<SessionResponse>, ApiError> {
    // step.1 验证消息内容和会话归属，阻止跨浏览器读取或操作会话。
    let client_id = client_id(&headers)?;
    let text = request.text.trim();
    if text.is_empty() || text.len() > 40_000 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "消息不能为空且不能超过 40000 个字节".to_owned(),
        ));
    }
    let session = owned_session(&state.database, &client_id, &session_id)?;
    if session.working {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "会话当前正在处理其他请求".to_owned(),
        ));
    }
    if !state
        .database
        .begin_turn(&client_id, &session_id, text)
        .map_err(internal_error)?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "会话当前正在处理其他请求".to_owned(),
        ));
    }
    spawn_turn(
        state.clone(),
        client_id.clone(),
        session_id.clone(),
        session.thread_id,
        text.to_owned(),
    );
    session_response(&state.database, &client_id, &session_id)
        .map(Json)
        .map_err(internal_error)
}

/// 中断当前浏览器身份拥有的会话中的 Codex 轮次。
async fn interrupt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let client_id = client_id(&headers)?;
    let session = owned_session(&state.database, &client_id, &session_id)?;
    state
        .codex
        .interrupt(&session.thread_id)
        .await
        .map_err(codex_error)?;
    Ok(Json(json!({ "status": "interrupted" })))
}

/// 验证会话归属并将批准或拒绝决定回传给原 App Server 连接。
async fn respond_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ApprovalDecision>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // step.1 校验用户身份、会话归属以及浏览器提交的审批选项。
    let client_id = client_id(&headers)?;
    let session = owned_session(&state.database, &client_id, &session_id)?;
    if !request.source_id.starts_with("approval:")
        || !matches!(request.decision.as_str(), "accept" | "decline")
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "审批决定无效".to_owned()));
    }
    let pending = state
        .database
        .list_activities(&session_id)
        .map_err(internal_error)?
        .iter()
        .any(|activity| {
            activity.source_id == request.source_id
                && activity.kind == "approval"
                && activity.status == "running"
        });
    if !pending {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "该审批请求已处理或已失效".to_owned(),
        ));
    }
    // step.2 确认请求仍在当前连接等待后，将决定送回 App Server。
    state
        .codex
        .respond_approval(&session.thread_id, &request.source_id, &request.decision)
        .await
        .map_err(codex_error)?;
    Ok(Json(json!({ "status": "submitted" })))
}

/// 返回日、周、月调用统计和本月用户用量排名。
async fn dashboard(State(state): State<AppState>) -> Result<Json<crate::db::Dashboard>, ApiError> {
    state.database.dashboard().map(Json).map_err(internal_error)
}

/// 从身份请求头中读取已由浏览器生成的稳定身份标识。
fn client_id(headers: &HeaderMap) -> Result<String, ApiError> {
    // 调用辅助函数校验身份字符集，防止身份被拼接成其他路径。
    crate::help::help_client_id(headers.get("x-client-id"))
        .map_err(|message| ApiError(StatusCode::BAD_REQUEST, message))
}

/// 验证会话表单字段长度并确保主题和需求说明有效。
fn validate_session(request: &NewSession) -> Result<(), ApiError> {
    // step.1 检查描述字段长度并确认关键任务内容不为空。
    let fields = [
        (&request.topic, "主题"),
        (&request.environment, "环境描述"),
        (&request.issue, "问题/需求描述"),
        (&request.work, "希望 Codex 完成的工作"),
        (&request.notes, "备注"),
    ];
    if request.issue.trim().is_empty() || request.work.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "问题/需求描述和希望 Codex 完成的工作不能为空".to_owned(),
        ));
    }
    if fields.iter().any(|(value, _)| value.len() > 20_000) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "每个会话说明字段不能超过 20000 个字节".to_owned(),
        ));
    }
    Ok(())
}

/// 验证会话归属并返回数据库中的会话记录。
fn owned_session(
    database: &Database,
    client_id: &str,
    session_id: &str,
) -> Result<crate::db::Session, ApiError> {
    database
        .get_session(client_id, session_id)
        .map_err(internal_error)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "找不到此会话".to_owned()))
}

/// 组装会话元数据和消息列表，并再次检查会话身份归属。
fn session_response(
    database: &Database,
    client_id: &str,
    session_id: &str,
) -> Result<SessionResponse> {
    let session = database
        .get_session(client_id, session_id)?
        .ok_or_else(|| anyhow::anyhow!("找不到此会话"))?;
    let messages = database.list_messages(session_id)?;
    let activities = database.list_activities(session_id)?;
    Ok(SessionResponse {
        session,
        messages,
        activities,
    })
}

/// 在后台接收 Codex 流式事件并持久化助手文本、工具活动和最终用量。
fn spawn_turn(
    state: AppState,
    client_id: String,
    session_id: String,
    thread_id: String,
    text: String,
) {
    // step.1 创建事件通道并安排活动写入任务。
    tokio::spawn(async move {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<CodexEvent>();
        let event_database = state.database.clone();
        let event_session = session_id.clone();
        let event_writer = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let result = persist_codex_event(&event_database, &event_session, event);
                if let Err(error) = result {
                    eprintln!("HXX 活动写入失败: {error}");
                }
            }
        });

        // step.1.1 显示 HXX 正在连接远程 App Server，避免等待期间对话框没有反馈。
        let _ = sender.send(CodexEvent {
            source_id: "hxx:connecting".to_owned(),
            kind: "connection".to_owned(),
            title: "正在连接 Codex App Server".to_owned(),
            detail: String::new(),
            status: "running".to_owned(),
            append_detail: false,
            assistant_delta: String::new(),
        });

        // step.2 运行远程 Codex 轮次，并等待所有已收到的增量持久化。
        let result = state.codex.send_turn(&thread_id, &text, sender).await;
        let _ = event_writer.await;

        // step.3 保存最终回复和 token 用量，或记录可重试的失败状态。
        match result {
            Ok(result) => {
                if let Err(error) = state.database.complete_turn(
                    &session_id,
                    &client_id,
                    &result.text,
                    result.input_tokens,
                    result.output_tokens,
                ) {
                    eprintln!("HXX 回复写入失败: {error}");
                }
            }
            Err(error) => {
                eprintln!("HXX Codex 轮次失败 session={session_id}: {error:#}");
                let _ = state.database.fail_turn(
                    &session_id,
                    &client_id,
                    &format!("Codex 请求失败：{error:#}"),
                );
            }
        }
    });
}

/// 将收到的 Codex 事件追加到助手消息或工具活动记录。
fn persist_codex_event(database: &Database, session_id: &str, event: CodexEvent) -> Result<()> {
    if !event.assistant_delta.is_empty() {
        return database.append_assistant_delta(session_id, &event.assistant_delta);
    }
    database.record_activity(
        session_id,
        &event.source_id,
        &event.kind,
        &event.title,
        &event.detail,
        &event.status,
        event.append_detail,
    )
}

/// 将数据库错误转换为服务器内部错误响应。
fn internal_error(error: anyhow::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

/// 将 App Server 连接或协议错误转换为网关错误响应。
fn codex_error(error: anyhow::Error) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, error.to_string())
}
