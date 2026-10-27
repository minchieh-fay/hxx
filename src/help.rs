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
