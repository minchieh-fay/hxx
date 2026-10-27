use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// 保存 HXX 用户、会话、消息和用量统计的数据模型。
pub struct Database {
    connection: Mutex<Connection>,
}

/// 描述浏览器对应的免登录用户资料。
#[derive(Serialize)]
pub struct Profile {
    pub client_id: String,
    pub name: String,
    pub client_ip: String,
}

/// 描述用户的一个 Codex 会话。
#[derive(Serialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing)]
    pub thread_id: String,
    pub working: bool,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 描述因用户会话数量超限而被移除的远程线程。
pub struct EvictedSession {
    pub thread_id: String,
}

/// 描述会话中的一条用户或 Codex 消息。
#[derive(Serialize)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// 描述 Codex 当前轮次的命令、工具和处理状态。
#[derive(Serialize)]
pub struct Activity {
    pub id: i64,
    pub source_id: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 描述一个时间范围内的调用次数和 token 用量。
#[derive(Serialize, Default)]
pub struct UsageTotals {
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 描述仪表盘中的用户用量排名项。
#[derive(Serialize)]
pub struct UsageRank {
    pub client_id: String,
    pub name: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 描述日、周、月统计和用户排名。
#[derive(Serialize)]
pub struct Dashboard {
    pub today: UsageTotals,
    pub week: UsageTotals,
    pub month: UsageTotals,
    pub rankings: Vec<UsageRank>,
}

/// 创建 SQLite 数据库并初始化 HXX 持久化表。
impl Database {
    /// 打开 SQLite 文件并准备 HXX 持久化表。
    pub fn open(path: &str) -> Result<Self> {
        // step.1 打开指定数据库文件并设置 SQLite 持久化模式。
        let connection = Connection::open(path).context("无法打开 HXX SQLite 数据库")?;

        // step.2 初始化用户、会话、对话和用量统计结构。
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS users (
               client_id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               client_ip TEXT NOT NULL,
               created_at TEXT NOT NULL,
               last_seen TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
               id TEXT PRIMARY KEY,
               client_id TEXT NOT NULL REFERENCES users(client_id),
               thread_id TEXT NOT NULL UNIQUE,
               title TEXT NOT NULL,
               cwd TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL REFERENCES sessions(id),
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS usage_events (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT,
               client_id TEXT NOT NULL REFERENCES users(client_id),
               requests INTEGER NOT NULL DEFAULT 1,
               input_tokens INTEGER NOT NULL DEFAULT 0,
               output_tokens INTEGER NOT NULL DEFAULT 0,
               created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_events (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL REFERENCES sessions(id),
               source_id TEXT NOT NULL,
               kind TEXT NOT NULL,
               title TEXT NOT NULL,
               detail TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               UNIQUE(session_id, source_id)
             );",
            )
            .context("无法初始化 HXX 数据库表")?;

        // step.3 升级用量表，使会话清理后历史统计仍可独立保留。
        let schema_version: i64 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if schema_version < 2 {
            connection
                .execute_batch(
                    "CREATE TABLE usage_events_v2 (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   session_id TEXT,
                   client_id TEXT NOT NULL REFERENCES users(client_id),
                   requests INTEGER NOT NULL DEFAULT 1,
                   input_tokens INTEGER NOT NULL DEFAULT 0,
                   output_tokens INTEGER NOT NULL DEFAULT 0,
                   created_at TEXT NOT NULL
                 );
                 INSERT INTO usage_events_v2(
                   id, session_id, client_id, requests, input_tokens, output_tokens, created_at
                 )
                 SELECT id, session_id, client_id, requests, input_tokens, output_tokens, created_at
                 FROM usage_events;
                 DROP TABLE usage_events;
                 ALTER TABLE usage_events_v2 RENAME TO usage_events;
                 PRAGMA user_version = 2;",
                )
                .context("无法迁移 HXX 用量记录表")?;
        }
        if schema_version < 3 {
            connection
                .execute_batch(
                    "ALTER TABLE sessions ADD COLUMN working INTEGER NOT NULL DEFAULT 0;
                     ALTER TABLE sessions ADD COLUMN last_error TEXT;
                     PRAGMA user_version = 3;",
                )
                .context("无法迁移 HXX 会话运行状态")?;
        }
        if schema_version < 4 {
            connection
                .execute_batch(
                    "ALTER TABLE session_events ADD COLUMN created_at TEXT NOT NULL DEFAULT '';
                     UPDATE session_events SET created_at = updated_at WHERE created_at = '';
                     PRAGMA user_version = 4;",
                )
                .context("无法迁移 HXX 活动时间")?;
        }

        // step.4 结束上次进程遗留的运行活动并清理会话标志。
        connection.execute(
            "UPDATE session_events SET status = 'interrupted', updated_at = ?1
             WHERE status = 'running'",
            [Utc::now().to_rfc3339()],
        )?;
        connection.execute(
            "UPDATE sessions SET
               working = 0,
               last_error = CASE
                 WHEN working = 1 THEN 'HXX 服务重启，请重新发送'
                 ELSE '上一轮没有完整回复，请继续发送消息'
               END
             WHERE working = 1 OR (
               SELECT role FROM messages WHERE session_id = sessions.id
               ORDER BY id DESC LIMIT 1
             ) = 'user'",
            [],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// 更新用户最近访问信息并返回当前资料。
    pub fn touch_profile(&self, client_id: &str, name: &str, ip: &str) -> Result<Profile> {
        // step.1 创建浏览器用户或更新最近访问时间和客户端 IP。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let now = Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO users(client_id, name, client_ip, created_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(client_id) DO UPDATE SET
               client_ip = excluded.client_ip,
               last_seen = excluded.last_seen",
            params![client_id, name, ip, now],
        )?;

        // step.2 查询并返回持久化后的用户名称和访问 IP。
        let profile = connection.query_row(
            "SELECT client_id, name, client_ip FROM users WHERE client_id = ?1",
            [client_id],
            |row| {
                Ok(Profile {
                    client_id: row.get(0)?,
                    name: row.get(1)?,
                    client_ip: row.get(2)?,
                })
            },
        )?;
        Ok(profile)
    }

    /// 保存用户主动修改的显示名称。
    pub fn rename_profile(&self, client_id: &str, name: &str, ip: &str) -> Result<Profile> {
        // step.1 更新用户名称、访问 IP 和最近活动时间。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let now = Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO users(client_id, name, client_ip, created_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(client_id) DO UPDATE SET
               name = excluded.name,
               client_ip = excluded.client_ip,
               last_seen = excluded.last_seen",
            params![client_id, name, ip, now],
        )?;
        // step.2 返回客户端刚提交的用户资料。
        Ok(Profile {
            client_id: client_id.to_owned(),
            name: name.to_owned(),
            client_ip: ip.to_owned(),
        })
    }

    /// 保存新会话的线程、工作目录和初始标题。
    pub fn create_session(
        &self,
        client_id: &str,
        id: &str,
        thread_id: &str,
        title: &str,
        cwd: &str,
    ) -> Result<Option<EvictedSession>> {
        // step.1 原子保存新会话并判断该用户是否超过会话上限。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let transaction = connection.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO sessions(id, client_id, thread_id, title, cwd, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, client_id, thread_id, title, cwd, now],
        )?;
        let session_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM sessions WHERE client_id = ?1",
            [client_id],
            |row| row.get(0),
        )?;
        let evicted = if session_count > 10 {
            transaction
                .query_row(
                    "SELECT id, thread_id FROM sessions
                     WHERE client_id = ?1
                     ORDER BY created_at ASC, rowid ASC LIMIT 1",
                    [client_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
        } else {
            None
        };

        // step.2 超限时删除最早会话及其消息，但保留独立的历史用量记录。
        if let Some((old_session_id, old_thread_id)) = evicted {
            transaction.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                [&old_session_id],
            )?;
            transaction.execute(
                "DELETE FROM session_events WHERE session_id = ?1",
                [&old_session_id],
            )?;
            transaction.execute("DELETE FROM sessions WHERE id = ?1", [&old_session_id])?;
            transaction.commit()?;
            return Ok(Some(EvictedSession {
                thread_id: old_thread_id,
            }));
        }
        transaction.commit()?;
        Ok(None)
    }

    /// 查询某个用户的会话列表并按最近活动时间排序。
    pub fn list_sessions(&self, client_id: &str) -> Result<Vec<Session>> {
        // step.1 只选择当前浏览器身份的有效会话。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let mut statement = connection.prepare(
            "SELECT id, title, thread_id, working, last_error, created_at, updated_at
             FROM sessions WHERE client_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = statement.query_map([client_id], |row| {
            Ok(Session {
                id: row.get(0)?,
                title: row.get(1)?,
                thread_id: row.get(2)?,
                working: row.get::<_, i64>(3)? != 0,
                last_error: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        // step.2 按最近活动顺序收集会话记录。
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("无法读取 HXX 会话列表")
    }

    /// 确认会话属于当前浏览器身份并返回会话元数据。
    pub fn get_session(&self, client_id: &str, session_id: &str) -> Result<Option<Session>> {
        // step.1 按浏览器身份和会话 ID 同时查询，避免跨身份访问。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        connection
            .query_row(
                "SELECT id, title, thread_id, working, last_error, created_at, updated_at
                 FROM sessions WHERE client_id = ?1 AND id = ?2",
                params![client_id, session_id],
                |row| {
                    Ok(Session {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        thread_id: row.get(2)?,
                        working: row.get::<_, i64>(3)? != 0,
                        last_error: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .context("无法读取 HXX 会话")
    }

    /// 原子地标记会话开始运行并保存用户消息。
    pub fn begin_turn(&self, client_id: &str, session_id: &str, content: &str) -> Result<bool> {
        // step.1 仅允许当前用户启动一个空闲会话轮次。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let transaction = connection.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        let updated = transaction.execute(
            "UPDATE sessions SET working = 1, last_error = NULL, updated_at = ?1
             WHERE id = ?2 AND client_id = ?3 AND working = 0",
            params![now, session_id, client_id],
        )?;
        if updated == 0 {
            return Ok(false);
        }

        // step.2 保存用户消息并提交运行状态，供前端立即显示。
        transaction.execute(
            "INSERT INTO messages(session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, "user", content, now],
        )?;
        transaction.execute(
            "INSERT INTO messages(session_id, role, content, created_at) VALUES (?1, ?2, '', ?3)",
            params![session_id, "assistant", now],
        )?;
        transaction.execute(
            "DELETE FROM session_events WHERE session_id = ?1",
            [session_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// 将 Codex 流式回复增量追加到当前助手消息。
    pub fn append_assistant_delta(&self, session_id: &str, delta: &str) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        connection.execute(
            "UPDATE messages SET content = content || ?1
             WHERE id = (
               SELECT id FROM messages WHERE session_id = ?2 AND role = 'assistant'
               ORDER BY id DESC LIMIT 1
             )",
            params![delta, session_id],
        )?;
        Ok(())
    }

    /// 新增或更新一条 Codex 工具活动，并限制活动详情的大小。
    pub fn record_activity(
        &self,
        session_id: &str,
        source_id: &str,
        kind: &str,
        title: &str,
        detail: &str,
        status: &str,
        append_detail: bool,
    ) -> Result<()> {
        // step.1 按 Codex 活动 ID 新增记录或更新正在运行的工具活动。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        connection.execute(
            "INSERT INTO session_events(
               session_id, source_id, kind, title, detail, status, updated_at, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(session_id, source_id) DO UPDATE SET
               kind = excluded.kind,
               title = excluded.title,
               detail = CASE
                 WHEN ?8 THEN substr(session_events.detail || excluded.detail, -16000)
                 WHEN excluded.detail != '' THEN excluded.detail
                 ELSE session_events.detail
               END,
               status = excluded.status,
               updated_at = excluded.updated_at",
            params![
                session_id,
                source_id,
                kind,
                title,
                detail,
                status,
                Utc::now().to_rfc3339(),
                append_detail
            ],
        )?;

        // step.2 限制持续输出大小并保存最新活动状态。
        Ok(())
    }

    /// 读取当前轮次最近的一百条工具活动。
    pub fn list_activities(&self, session_id: &str) -> Result<Vec<Activity>> {
        // step.1 按生成顺序读取当前会话的最近工具活动。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let mut statement = connection.prepare(
            "SELECT id, source_id, kind, title, detail, status, created_at, updated_at FROM (
               SELECT id, source_id, kind, title, detail, status, created_at, updated_at
               FROM session_events WHERE session_id = ?1
               ORDER BY id DESC LIMIT 100
             ) ORDER BY id ASC",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok(Activity {
                id: row.get(0)?,
                source_id: row.get(1)?,
                kind: row.get(2)?,
                title: row.get(3)?,
                detail: row.get(4)?,
                status: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;

        // step.2 将数据库记录整理为对话框中的活动列表。
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("无法读取 Codex 活动记录")
    }

    /// 读取会话对话记录并按发送顺序返回。
    pub fn list_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        // step.1 查询该会话内的所有消息并保持数据库发送顺序。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let mut statement = connection.prepare(
            "SELECT id, role, content, created_at FROM messages
             WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("无法读取 HXX 对话记录")
    }

    /// 保存 Codex 回复和本轮 token 用量并结束运行状态。
    pub fn complete_turn(
        &self,
        session_id: &str,
        client_id: &str,
        content: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<()> {
        // step.1 在同一事务中保存助手回复和请求用量。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let transaction = connection.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE messages SET content = ?1 WHERE id = (
               SELECT id FROM messages WHERE session_id = ?2 AND role = 'assistant'
               ORDER BY id DESC LIMIT 1
             )",
            params![content, session_id],
        )?;
        transaction.execute(
            "INSERT INTO usage_events(
               session_id, client_id, input_tokens, output_tokens, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                client_id,
                input_tokens.max(0),
                output_tokens.max(0),
                now
            ],
        )?;

        // step.2 清除运行标志并提交最终会话状态。
        transaction.execute(
            "UPDATE sessions SET working = 0, last_error = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )?;
        transaction.execute(
            "UPDATE session_events SET status = 'completed', updated_at = ?1
             WHERE session_id = ?2 AND status = 'running'",
            params![now, session_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// 记录失败请求并清除运行状态，让用户可以重试。
    pub fn fail_turn(&self, session_id: &str, client_id: &str, error: &str) -> Result<()> {
        // step.1 记录失败请求次数并清除当前会话的运行状态。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let transaction = connection.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO usage_events(session_id, client_id, input_tokens, output_tokens, created_at)
             VALUES (?1, ?2, 0, 0, ?3)",
            params![session_id, client_id, now],
        )?;
        transaction.execute(
            "UPDATE sessions SET working = 0, last_error = ?1, updated_at = ?2 WHERE id = ?3",
            params![error, now, session_id],
        )?;
        transaction.execute(
            "UPDATE session_events SET status = 'failed', updated_at = ?1
             WHERE session_id = ?2 AND status = 'running'",
            params![now, session_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// 汇总当天、本周、本月用量并计算本月用户排名。
    pub fn dashboard(&self) -> Result<Dashboard> {
        // step.1 按本机时区计算日历周期边界并读取用量汇总。
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁异常"))?;
        let local_now = Local::now();
        let local_date = local_now.date_naive();
        let today = local_midnight(local_date)?;
        let week_date =
            local_date - Duration::days(i64::from(local_date.weekday().num_days_from_monday()));
        let week = local_midnight(week_date)?;
        let month_date = local_date.with_day(1).context("无法计算本月起始日期")?;
        let month = local_midnight(month_date)?;
        let today_totals = query_totals(&connection, today)?;
        let week_totals = query_totals(&connection, week)?;
        let month_totals = query_totals(&connection, month)?;

        // step.2 按本月请求数生成用户排名。
        let rankings = query_rankings(&connection, month)?;
        Ok(Dashboard {
            today: today_totals,
            week: week_totals,
            month: month_totals,
            rankings,
        })
    }
}

/// 将本地日历日期的午夜转换成 UTC 统计边界。
fn local_midnight(date: NaiveDate) -> Result<DateTime<Utc>> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .context("无法计算本地日期起始时间")?;
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .context("无法解析本地日期起始时间")
        .map(|value| value.with_timezone(&Utc))
}

/// 查询指定起点之后的调用次数和 token 汇总。
fn query_totals(connection: &Connection, from: DateTime<Utc>) -> Result<UsageTotals> {
    let mut statement = connection.prepare(
        "SELECT COALESCE(SUM(requests), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0)
         FROM usage_events WHERE created_at >= ?1",
    )?;
    let totals = statement.query_row([from.to_rfc3339()], |row| {
        Ok(UsageTotals {
            requests: row.get(0)?,
            input_tokens: row.get(1)?,
            output_tokens: row.get(2)?,
        })
    })?;
    Ok(totals)
}

/// 查询指定时间范围内按请求次数排序的用户用量。
fn query_rankings(connection: &Connection, from: DateTime<Utc>) -> Result<Vec<UsageRank>> {
    // step.1 将用户资料与用量记录聚合，按请求数和 token 总量排序。
    let mut statement = connection.prepare(
        "SELECT u.client_id, u.name, SUM(e.requests),
                SUM(e.input_tokens), SUM(e.output_tokens)
         FROM usage_events e JOIN users u ON u.client_id = e.client_id
         WHERE e.created_at >= ?1
         GROUP BY u.client_id, u.name
         ORDER BY SUM(e.requests) DESC, SUM(e.input_tokens + e.output_tokens) DESC
         LIMIT 100",
    )?;
    let rows = statement.query_map([from.to_rfc3339()], |row| {
        Ok(UsageRank {
            client_id: row.get(0)?,
            name: row.get(1)?,
            requests: row.get(2)?,
            input_tokens: row.get(3)?,
            output_tokens: row.get(4)?,
        })
    })?;

    // step.2 限制排名结果数量并转换为前端展示结构。
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("无法读取 HXX 用量排名")
}
