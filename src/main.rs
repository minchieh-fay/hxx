mod api;
mod audit;
mod codex;
mod config;
mod db;
mod help;

use std::{net::SocketAddr, path::Path, sync::Arc};

use api::AppState;
use audit::AuditLog;
use config::HxxConfig;
use db::Database;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

/// 启动本地 HXX API、SQLite 存储和 Vue 静态页面服务。
#[tokio::main]
async fn main() {
    // step.1 载入统一 TOML 配置并准备持久化数据库。
    let config = HxxConfig::load().expect("无法读取 HXX 配置");
    println!(
        "HXX protocol audit enabled={} include_payloads={} directory={}",
        config.audit.enabled,
        config.audit.include_payloads,
        config.audit.directory.display()
    );
    let audit = AuditLog::new(config.audit.clone());
    if let Some(parent) = Path::new(&config.server.database_path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("无法创建 HXX 数据目录");
        }
    }
    let database = Database::open(&config.server.database_path).expect("无法初始化 HXX 数据库");

    // step.2 组合业务 API 和由 Rust 托管的前端构建产物。
    let state = AppState {
        database: Arc::new(database),
        codex: codex::CodexClient::new(config.codex.clone(), audit),
    };
    let frontend = ServeDir::new("frontend/dist")
        .not_found_service(ServeFile::new("frontend/dist/index.html"));
    let app = api::routes(state).fallback_service(frontend);

    // step.3 启动本地 HTTP 服务并提供客户端 IP 给用户资料接口。
    let listener = TcpListener::bind(&config.server.bind)
        .await
        .expect("无法绑定 HXX HTTP 服务地址");
    println!("HXX listening on {}", config.server.bind);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("HXX HTTP 服务异常退出");
}
