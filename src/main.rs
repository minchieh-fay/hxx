mod api;
mod audit;
mod codex;
mod config;
mod db;
mod help;

use std::{env, net::SocketAddr, path::Path, sync::Arc};

use api::AppState;
use audit::AuditLog;
use config::HxxConfig;
use db::Database;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

/// 启动本地 HXX API、SQLite 存储和 Vue 静态页面服务。
#[tokio::main]
async fn main() {
    // step.1 载入本地忽略配置并准备持久化数据库。
    let _ = dotenvy::dotenv();
    let config = HxxConfig::load().expect("无法读取 HXX 配置");
    println!(
        "HXX protocol audit enabled={} include_payloads={} directory={}",
        config.audit.enabled,
        config.audit.include_payloads,
        config.audit.directory.display()
    );
    let audit = AuditLog::new(config.audit.clone());
    let database_path =
        env::var("HXX_DATABASE_PATH").unwrap_or_else(|_| "data/hxx.sqlite".to_owned());
    if let Some(parent) = Path::new(&database_path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("无法创建 HXX 数据目录");
        }
    }
    let database = Database::open(&database_path).expect("无法初始化 HXX 数据库");

    // step.2 组合业务 API 和由 Rust 托管的前端构建产物。
    let state = AppState {
        database: Arc::new(database),
        codex: codex::CodexClient::from_env(audit),
    };
    let frontend = ServeDir::new("frontend/dist")
        .not_found_service(ServeFile::new("frontend/dist/index.html"));
    let app = api::routes(state).fallback_service(frontend);

    // step.3 启动本地 HTTP 服务并提供客户端 IP 给用户资料接口。
    let bind_address = env::var("HXX_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let listener = TcpListener::bind(&bind_address)
        .await
        .expect("无法绑定 HXX HTTP 服务地址");
    println!("HXX listening on {bind_address}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("HXX HTTP 服务异常退出");
}
