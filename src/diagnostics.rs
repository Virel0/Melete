//! Reports apps send about themselves: what went wrong, in fields an admin can read,
//! kept per device for a while instead of piling up as text files.
//!
//! A report is a few named fields and a JSON object of details. Anything in it that looks like
//! a secret (a value under a key such as `password` or `token`, or one of Melete's own tokens
//! anywhere in a string) is replaced before it's stored, so a careless app can't leave one here.

use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::SqlitePool;

/// How long a report is kept.
pub const KEEP_FOR: i64 = 30 * 24 * 3600;
/// Reports kept per device; older ones go first.
pub const PER_DEVICE: i64 = 100;
/// The largest report body taken; a bigger one is refused whole.
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 32;
const REDACTED: &str = "[redacted]";

/// A report as an app sends it. Unknown fields are refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// Which app sent it: 1 to 64 characters.
    pub app: String,
    /// The app's version: at most 32 characters.
    pub app_version: Option<String>,
    /// What it runs on: at most 64 characters.
    pub platform: Option<String>,
    /// "info" (the default), "warning" or "error".
    #[serde(default = "info")]
    pub level: String,
    /// What happened, in a line: 1 to 500 characters.
    pub summary: String,
    /// Anything else, nested at most 32 levels; secrets in it are replaced before it's kept.
    #[serde(default)]
    pub details: Map<String, Value>,
}

fn info() -> String {
    "info".into()
}

/// Checks a report, or says what's wrong with it.
fn check(report: &Report) -> Result<(), String> {
    let within =
        |value: &str, most: usize| !value.trim().is_empty() && value.chars().count() <= most;
    if !within(&report.app, 64) {
        return Err("app is 1 to 64 characters".into());
    }
    if report
        .app_version
        .as_deref()
        .is_some_and(|v| v.chars().count() > 32)
        || report
            .platform
            .as_deref()
            .is_some_and(|p| p.chars().count() > 64)
    {
        return Err("app_version is at most 32 characters and platform at most 64".into());
    }
    if !["info", "warning", "error"].contains(&report.level.as_str()) {
        return Err("level is info, warning or error".into());
    }
    if !within(&report.summary, 500) {
        return Err("summary is 1 to 500 characters".into());
    }
    if depth(&Value::Object(report.details.clone())) > MAX_DEPTH {
        return Err(format!("details nest at most {MAX_DEPTH} levels deep"));
    }
    Ok(())
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Whether a key names something secret, case and separators aside.
fn secret_key(key: &str) -> bool {
    let key: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "password",
        "passwd",
        "token",
        "secret",
        "apikey",
        "authorization",
        "cookie",
        "credential",
    ]
    .iter()
    .any(|word| key.contains(word))
}

/// `text` with Melete's tokens (session and API keys, stream links) replaced.
fn scrub_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = ["mlt_", "mls_"]
        .iter()
        .filter_map(|prefix| rest.find(prefix))
        .min()
    {
        out.push_str(&rest[..at]);
        let after = &rest[at + 4..];
        let token_len = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
        if token_len >= 16 {
            out.push_str(REDACTED);
        } else {
            out.push_str(&rest[at..at + 4 + token_len]);
        }
        rest = &after[token_len..];
    }
    out.push_str(rest);
    out
}

fn scrub(value: &mut Value) {
    match value {
        Value::String(text) => *text = scrub_text(text),
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if secret_key(key) && !value.is_null() {
                    *value = Value::String(REDACTED.into());
                } else {
                    scrub(value);
                }
            }
        }
        _ => {}
    }
}

/// Keeps a report from the device `token_id` stands for, then trims that device's reports to
/// the newest [`PER_DEVICE`]. `Err` says what's wrong with the report.
pub async fn keep(
    pool: &SqlitePool,
    user_id: i64,
    token_id: i64,
    mut report: Report,
    now: i64,
) -> anyhow::Result<Result<i64, String>> {
    if let Err(why) = check(&report) {
        return Ok(Err(why));
    }
    let mut details = Value::Object(std::mem::take(&mut report.details));
    scrub(&mut details);
    let device: String = sqlx::query_scalar("SELECT name FROM tokens WHERE id = ?")
        .bind(token_id)
        .fetch_one(pool)
        .await?;
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO diagnostics (user_id, token_id, device, app, app_version, platform, level,
             summary, details, received_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(token_id)
    .bind(&device)
    .bind(report.app.trim())
    .bind(report.app_version.as_deref().map(str::trim))
    .bind(report.platform.as_deref().map(str::trim))
    .bind(&report.level)
    .bind(scrub_text(report.summary.trim()))
    .bind(details.to_string())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM diagnostics WHERE user_id = ?1 AND device = ?2 AND id NOT IN (
             SELECT id FROM diagnostics WHERE user_id = ?1 AND device = ?2
             ORDER BY received_at DESC, id DESC LIMIT ?3)",
    )
    .bind(user_id)
    .bind(&device)
    .bind(PER_DEVICE)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

/// Deletes reports older than [`KEEP_FOR`]; how many went.
pub async fn prune(pool: &SqlitePool, now: i64) -> anyhow::Result<u64> {
    Ok(sqlx::query("DELETE FROM diagnostics WHERE received_at < ?")
        .bind(now - KEEP_FOR)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    fn report(body: Value) -> Report {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn secrets_are_replaced_wherever_they_are() {
        let token = format!("mlt_{}", "a1".repeat(32));
        let mut details = json!({
            "Password": "hunter22",
            "api-key": "abc",
            "headers": { "Authorization": format!("Bearer {token}"), "Accept": "*/*" },
            "url": format!("/api/v1/stream/mls_{}/x", "f".repeat(64)),
            "log": [format!("signed in with {token} ok"), "mlt_short stays"],
            "session_token": null,
            "count": 3,
        });
        scrub(&mut details);
        assert_eq!(
            details,
            json!({
                "Password": "[redacted]",
                "api-key": "[redacted]",
                "headers": { "Authorization": "[redacted]", "Accept": "*/*" },
                "url": "/api/v1/stream/[redacted]/x",
                "log": ["signed in with [redacted] ok", "mlt_short stays"],
                "session_token": null,
                "count": 3,
            })
        );
    }

    #[test]
    fn reports_are_checked() {
        let fine = json!({ "app": "Video app", "summary": "Playback stalled" });
        assert_eq!(check(&report(fine.clone())), Ok(()));
        let mut deep = json!(1);
        for _ in 0..40 {
            deep = json!([deep]);
        }
        for (change, expected) in [
            (json!({ "app": " " }), "app is"),
            (json!({ "app": "x".repeat(65) }), "app is"),
            (json!({ "summary": "" }), "summary is"),
            (json!({ "summary": "x".repeat(501) }), "summary is"),
            (json!({ "level": "fatal" }), "level is"),
            (json!({ "platform": "x".repeat(65) }), "platform"),
            (json!({ "app_version": "x".repeat(33) }), "app_version"),
            (json!({ "details": { "deep": deep } }), "nest"),
        ] {
            let mut body = fine.clone();
            for (k, v) in change.as_object().unwrap() {
                body[k] = v.clone();
            }
            let why = check(&report(body)).unwrap_err();
            assert!(why.contains(expected), "{change}: {why}");
        }
        assert!(
            serde_json::from_value::<Report>(json!({ "app": "a", "summary": "s", "extra": 1 }))
                .is_err(),
            "unknown fields are refused"
        );
    }

    #[tokio::test]
    async fn kept_per_device_newest_first_and_pruned_after_thirty_days() {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (phone, _) = auth::issue_token(&pool, user.id, TokenKind::Session, "Phone")
            .await
            .unwrap();
        let (tv, _) = auth::issue_token(&pool, user.id, TokenKind::Session, "Living room TV")
            .await
            .unwrap();
        let body = || report(json!({ "app": "Video app", "summary": "Stalled" }));
        for n in 0..PER_DEVICE + 5 {
            keep(&pool, user.id, phone, body(), 1000 + n)
                .await
                .unwrap()
                .unwrap();
        }
        keep(&pool, user.id, tv, body(), 5000)
            .await
            .unwrap()
            .unwrap();
        let per_device: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT device, count(*), min(received_at) FROM diagnostics GROUP BY device
             ORDER BY device",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            per_device,
            [
                ("Living room TV".into(), 1, 5000),
                ("Phone".into(), PER_DEVICE, 1005)
            ]
        );
        let gone = prune(&pool, 1050 + KEEP_FOR).await.unwrap();
        assert_eq!(gone, 45, "older than 30 days");
        let bad = report(json!({ "app": "Video app", "summary": "" }));
        assert!(keep(&pool, user.id, tv, bad, 1).await.unwrap().is_err());
    }
}
