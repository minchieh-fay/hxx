use std::env;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

/// 启动 HXX HTTP 服务，并提供健康接口与前端静态资源。
#[tokio::main]
async fn main() {
    // step.1 组装健康接口和前端静态资源路由。
    let frontend = ServeDir::new("frontend/dist")
        .not_found_service(ServeFile::new("frontend/dist/index.html"));
    let app = Router::new()
        .route("/api/health", get(health))
        .fallback_service(frontend);

    // step.2 读取监听地址并启动 HTTP 服务。
    let bind_address = env::var("HXX_BIND").unwrap_or_else(|_| "0.0.0.0:3000".to_owned());
    let listener = TcpListener::bind(&bind_address)
        .await
        .expect("无法绑定 HXX HTTP 服务地址");
    println!("HXX listening on {bind_address}");
    axum::serve(listener, app)
        .await
        .expect("HXX HTTP 服务异常退出");
}

/// 描述健康检查接口返回的数据。
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// 返回 HXX Web 后端的健康状态。
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
