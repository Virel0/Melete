//! The downloads of the wanted titles in a list, gathered once per list.

use std::collections::{HashMap, HashSet};

use super::*;

/// The downloads of one kind of wanted title: the one under way for each, and which ones'
/// newest download was imported.
pub(crate) struct Downloads {
    under_way: HashMap<i64, DownloadNow>,
    imported: HashSet<i64>,
}

/// The downloads under way, and the newest of each wanted title, whose `column` (a constant)
/// names one.
pub(super) fn query(column: &str) -> String {
    format!(
        "SELECT {column}, id, state, progress, message FROM downloads
         WHERE {column} IS NOT NULL
           AND (state IN {UNDER_WAY}
                OR id IN (SELECT max(id) FROM downloads WHERE {column} IS NOT NULL
                          GROUP BY {column}))
         ORDER BY id"
    )
}

impl Downloads {
    /// The downloads whose `column` names a wanted title.
    pub(crate) async fn load(pool: &SqlitePool, column: &'static str) -> anyhow::Result<Self> {
        let rows: Vec<(i64, i64, String, f64, Option<String>)> =
            sqlx::query_as(sqlx::AssertSqlSafe(query(column)))
                .fetch_all(pool)
                .await?;
        let mut under_way = HashMap::new();
        let mut newest: HashMap<i64, String> = HashMap::new();
        for (wanted, id, state, progress, message) in rows {
            if matches!(state.as_str(), "downloading" | "completed" | "transferring") {
                // The newest under way wins.
                under_way.insert(
                    wanted,
                    DownloadNow::new(id, &state, progress, message.as_deref()),
                );
            }
            newest.insert(wanted, state);
        }
        let imported = newest
            .into_iter()
            .filter(|(_, state)| state == "imported")
            .map(|(wanted, _)| wanted)
            .collect();
        Ok(Downloads {
            under_way,
            imported,
        })
    }

    /// The download under way for a wanted title.
    pub(crate) fn under_way(&self, wanted: i64) -> Option<DownloadNow> {
        self.under_way.get(&wanted).cloned()
    }

    /// Whether the wanted title's newest download was imported (and may wait for a scan).
    pub(crate) fn imported(&self, wanted: i64) -> bool {
        self.imported.contains(&wanted)
    }
}
