use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 保存 HXX 本地服务运行时配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct HxxConfig {
    pub audit: AuditConfig,
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
            audit: AuditConfig::default(),
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
        // step.1 读取环境变量指定的配置路径或默认本地配置文件。
        let path = env::var("HXX_CONFIG_PATH").unwrap_or_else(|_| "hxx.toml".to_owned());
        let contents = match std::fs::read_to_string(&path) {
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
