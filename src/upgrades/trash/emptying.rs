//! Emptying the whole trash in the background: each file deleted one after another as
//! [`super::delete_row`] does, with how far it has got kept for the trash page to show. A delete
//! stuck on its share holds up neither the page nor the files after it.

use std::sync::{Mutex, PoisonError};

use super::*;

/// How emptying the trash is going, or how it last went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Emptying {
    /// Whether it's under way.
    pub running: bool,
    /// Files in the trash when it started.
    pub files: usize,
    /// Of those, deleted so far.
    pub deleted: usize,
    /// Left in place: changed since the trash put them there, or their library isn't
    /// responding.
    pub left: usize,
    /// Still being deleted after the time limit: recorded once their share answers.
    pub waiting: usize,
    /// Bytes freed so far.
    pub freed: i64,
    /// The file being deleted now (its path in its library), while running.
    pub current: Option<String>,
    /// When it started and finished, in Unix seconds; None before the first run.
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

static STATE: Mutex<Emptying> = Mutex::new(Emptying {
    running: false,
    files: 0,
    deleted: 0,
    left: 0,
    waiting: 0,
    freed: 0,
    current: None,
    started_at: None,
    finished_at: None,
});

fn state() -> std::sync::MutexGuard<'static, Emptying> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How emptying the trash is going, or how it last went since the server started.
pub fn emptying() -> Emptying {
    state().clone()
}

/// Starts emptying the whole trash in the background, unless that's under way already; answers
/// how it's going.
pub async fn start_emptying(pool: &SqlitePool) -> anyhow::Result<Emptying> {
    {
        let mut now = state();
        if now.running {
            return Ok(now.clone());
        }
        *now = Emptying {
            running: true,
            started_at: Some(unix_now()),
            ..Emptying::default()
        };
    }
    let rows: sqlx::Result<Vec<TrashRow>> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{ROWS} ORDER BY t.id")))
            .fetch_all(pool)
            .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            let mut now = state();
            now.running = false;
            now.finished_at = Some(unix_now());
            return Err(err.into());
        }
    };
    state().files = rows.len();
    tracing::info!(files = rows.len(), "emptying the trash");
    let pool = pool.clone();
    tokio::spawn(async move {
        for row in rows {
            let (size, path) = (row.size, row.trash_path.clone());
            state().current = Some(path);
            let deleted = delete_row(&pool, row).await;
            let mut now = state();
            match deleted {
                Ok(Deletion::Gone) => {
                    now.deleted += 1;
                    now.freed += size;
                }
                Ok(Deletion::Left(_)) => now.left += 1,
                Ok(Deletion::Waiting) => now.waiting += 1,
                Err(err) => {
                    tracing::warn!(error = format!("{err:#}"), "couldn't delete from the trash");
                    now.left += 1;
                }
            }
        }
        let mut now = state();
        now.running = false;
        now.current = None;
        now.finished_at = Some(unix_now());
        tracing::info!(
            deleted = now.deleted,
            left = now.left,
            waiting = now.waiting,
            freed = now.freed,
            "emptied the trash"
        );
    });
    Ok(emptying())
}
