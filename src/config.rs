use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// 保存 HXX 本地服务运行时配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct HxxConfig {
    pub server: ServerConfig,
    pub codex: CodexConfig,
    pub audit: AuditConfig,
}

/// 描述 HXX HTTP 服务与 SQLite 数据库的位置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub database_path: String,
}

/// 描述 Codex 连接认证和会话工作目录的准备方式。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CodexConfig {
    pub ws_url: String,
    pub token_file: PathBuf,
    pub workspace_ssh_target: Option<String>,
    pub workspace_ssh_root: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
}

/// 描述协议审计的启用状态、文件目录和消息正文记录开关。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub enabled: bool,
    pub directory: PathBuf,
    pub include_payloads: bool,
}

/// 提供本地调试默认审计设置。
impl Default for HxxConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            codex: CodexConfig::default(),
            audit: AuditConfig::default(),
        }
    }
}

/// 提供本机开发服务的默认监听地址和数据库路径。
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3000".to_owned(),
            database_path: "data/hxx.sqlite".to_owned(),
        }
    }
}

/// 提供 Codex App Server 和本机会话工作区的默认设置。
impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            ws_url: "ws://127.0.0.1:4500".to_owned(),
            token_file: PathBuf::from("codex/secrets/app-server-token"),
            workspace_ssh_target: None,
            workspace_ssh_root: None,
            workspace_root: None,
        }
    }
}

/// 默认记录协议元数据，并把正文放在显式配置开关后。
impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("data/audit"),
            include_payloads: false,
        }
    }
}

/// 从本地 TOML 文件读取配置，文件不存在时使用调试默认值。
impl HxxConfig {
    pub fn load() -> Result<Self> {
        // step.1 从当前工作目录读取统一 TOML 配置文件。
        let path = "hxx.toml";
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).with_context(|| format!("无法读取 HXX 配置 {path}")),
        };

        // step.2 解析并校验 TOML 配置内容。
        toml::from_str(&contents).with_context(|| format!("HXX 配置格式无效: {path}"))
    }
}
