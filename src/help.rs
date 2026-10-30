/// 从请求头中读取浏览器身份标识。
pub fn help_client_id(value: Option<&axum::http::HeaderValue>) -> Result<String, String> {
    let raw = value
        .ok_or_else(|| "缺少浏览器身份标识".to_owned())?
        .to_str()
        .map_err(|_| "浏览器身份标识格式无效".to_owned())?;
    let id = raw.trim();
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("浏览器身份标识格式无效".to_owned());
    }
    Ok(id.to_owned())
}

/// 将会话说明整理为发给 Codex 的 Markdown 首条消息。
pub fn help_build_session_prompt(
    topic: &str,
    environment: &str,
    issue: &str,
    work: &str,
    notes: &str,
) -> String {
    format!(
        "# 主题\n{topic}\n\n# 环境描述\n{environment}\n\n# 问题/需求描述\n{issue}\n\n# 希望 Codex 完成的工作\n{work}\n\n# 备注\n{notes}"
    )
}

/// 从用户文件名中提取安全的工作区文件名，避免目录穿越和空名称。
pub fn help_safe_attachment_name(name: &str) -> String {
    let basename = std::path::Path::new(name)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("attachment");
    let cleaned = basename
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .collect::<String>();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "attachment".to_owned()
    } else {
        cleaned
    }
}

/// 为同一轮上传中的同名文件生成不冲突的工作区文件名。
pub fn help_unique_attachment_name(
    name: &str,
    used_names: &mut std::collections::HashSet<String>,
) -> String {
    // step.1 优先保留不冲突的原文件名。
    if used_names.insert(name.to_owned()) {
        return name.to_owned();
    }
    // step.2 为同名文件依次生成带编号的候选名称。
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("attachment");
    let extension = path.extension().and_then(std::ffi::OsStr::to_str);
    let mut index = 2;
    loop {
        let candidate = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

/// 将上传附件清单追加到发给 Codex 的 Markdown 消息中。
pub fn help_attachment_prompt(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let entries = names
        .iter()
        .map(|name| format!("- `{name}`"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n# 附件\n已放入当前会话工作目录，请结合任务查看：\n{entries}")
}

/// 从 App Server 消息中提取当前轮次的输入和输出 token 数。
pub fn help_token_counts(value: &serde_json::Value) -> (i64, i64) {
    let usage = value
        .pointer("/params/tokenUsage/last")
        .or_else(|| value.pointer("/tokenUsage/last"))
        .or_else(|| value.pointer("/result/tokenUsage/last"));
    let Some(usage) = usage else {
        return (0, 0);
    };
    let input = usage
        .get("inputTokens")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("outputTokens")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    (input.max(0), output.max(0))
}

/// 从 App Server 线程历史中查找指定轮次的完整记录。
pub fn help_find_turn_history(
    history: &serde_json::Value,
    turn_id: &str,
) -> Option<serde_json::Value> {
    history
        .get("data")?
        .as_array()?
        .iter()
        .find(|turn| turn.get("id").and_then(serde_json::Value::as_str) == Some(turn_id))
        .cloned()
}

/// 汇总线程历史中 Codex 明确发给用户的助手消息文本。
pub fn help_turn_agent_text(turn: &serde_json::Value) -> String {
    turn.get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("agentMessage"))
        .filter_map(|item| item.get("text").and_then(serde_json::Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}
