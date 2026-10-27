use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::{fs, io::AsyncWriteExt, sync::Mutex};

use crate::config::AuditConfig;

/// 将 Codex App Server 的 JSON-RPC 收发记录追加到本地 JSONL 审计文件。
#[derive(Clone)]
pub struct AuditLog {
    config: Arc<AuditConfig>,
    write_lock: Arc<Mutex<()>>,
}

/// 创建共享的协议审计记录器。
impl AuditLog {
    pub fn new(config: AuditConfig) -> Self {
        Self {
            config: Arc::new(config),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 追加一条带方向、方法、请求编号和可选消息正文的审计记录。
    pub async fn record(
        &self,
        direction: &str,
        rpc_method: Option<&str>,
        thread_id: Option<&str>,
        message: &Value,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // step.1 提取协议元数据，并按配置决定是否保存原始 JSON 正文。
        let now = Utc::now();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .or(rpc_method)
            .unwrap_or("response");
        let payload_bytes = serde_json::to_vec(message)?.len();
        let entry = json!({
            "timestamp": now.to_rfc3339(),
            "direction": direction,
            "method": method,
            "request_id": message.get("id"),
            "thread_id": thread_id.or_else(|| {
                message.pointer("/params/threadId").and_then(Value::as_str)
            }),
            "event_status": message.pointer("/params/status/type")
                .or_else(|| message.pointer("/params/turn/status")),
            "error": message.get("error"),
            "payload_bytes": payload_bytes,
            "payload": self.config.include_payloads.then_some(message),
        });
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');

        // step.2 串行追加到按日期分割的 JSONL 文件，避免并发写入交错。
        let _guard = self.write_lock.lock().await;
        let path = self.log_path(now);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .context("无法创建 HXX 审计日志目录")?;
        }
        let mut file = open_audit_file(&path).await?;
        file.write_all(&line)
            .await
            .context("无法追加 HXX 审计记录")?;
        Ok(())
    }

    /// 返回当前日期对应的审计日志路径。
    fn log_path(&self, now: chrono::DateTime<Utc>) -> PathBuf {
        self.config
            .directory
            .join(format!("{}.jsonl", now.format("%Y-%m-%d")))
    }
}

/// 以仅当前用户可读写的权限打开审计文件。
async fn open_audit_file(path: &std::path::Path) -> Result<fs::File> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .await
        .with_context(|| format!("无法打开 HXX 审计文件 {}", path.display()))
}
