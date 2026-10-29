//! Working through imports in the background, one at a time and a track at a time.

use super::*;

/// One import at a time: they share MusicBrainz's one request a second.
pub(super) static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Works through imports for as long as the server runs, taking up any a restart cut off.
pub async fn work(pool: SqlitePool, mb: Arc<musicbrainz::Client>, sources: fulfil::Sources) {
    loop {
        let next: Result<Option<i64>, _> = sqlx::query_scalar(
            "SELECT id FROM playlist_imports WHERE state IN ('queued', 'running') ORDER BY id LIMIT 1",
        )
        .fetch_optional(&pool)
        .await;
        match next {
            Ok(Some(id)) => {
                if let Err(err) = run(&pool, &mb, &sources, id).await {
                    tracing::warn!(
                        import = id,
                        error = format!("{err:#}"),
                        "a playlist import failed"
                    );
                    let _ = sqlx::query(
                        "UPDATE playlist_imports SET state = 'failed', message = ?, finished_at = ?
                         WHERE id = ?",
                    )
                    .bind(format!("{err:#}"))
                    .bind(unix_now())
                    .bind(id)
                    .execute(&pool)
                    .await;
                }
            }
            Ok(None) => tokio::time::sleep(IDLE).await,
            Err(err) => {
                tracing::warn!(error = %err, "couldn't look for playlist imports");
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

/// Matches and asks for each track still pending, in order, fetching each one asked for at
/// once before going on.
pub async fn run(
    pool: &SqlitePool,
    mb: &musicbrainz::Client,
    sources: &fulfil::Sources,
    id: i64,
) -> anyhow::Result<()> {
    let _turn = RUNNING.lock().await;
    let Some(user) = owner(pool, id).await? else {
        return Ok(());
    };
    sqlx::query("UPDATE playlist_imports SET state = 'running' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let pending = pending(pool, id).await?;
    for (position, title, artist, album, isrc, length_ms) in pending {
        // Removed partway: nothing more is asked for.
        let still: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM playlist_imports WHERE id = ?)")
                .bind(id)
                .fetch_one(pool)
                .await?;
        if !still {
            return Ok(());
        }
        let track = Track {
            title,
            artists: artist
                .map(|a| a.split(", ").map(str::to_owned).collect())
                .unwrap_or_default(),
            album,
            length_ms,
            isrc,
        };
        let (state, why, request, fetch) = ask(pool, mb, &user, &track).await?;
        sqlx::query(
            "UPDATE playlist_import_tracks SET state = ?, why = ?, request_id = ?
             WHERE import_id = ? AND position = ?",
        )
        .bind(state)
        .bind(&why)
        .bind(request)
        .bind(id)
        .bind(position)
        .execute(pool)
        .await?;
        if let (true, Some(request)) = (fetch, request)
            && let Err(err) = fulfil::run(pool, sources, request).await
        {
            tracing::warn!(
                request,
                error = format!("{err:#}"),
                "couldn't fetch an imported track"
            );
        }
    }
    finish(pool, id).await
}

/// The user who imported the playlist, whose requests its tracks become.
async fn owner(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<User>> {
    let owner: Option<(i64, String, bool)> = sqlx::query_as(
        "SELECT u.id, u.username, u.is_admin FROM playlist_imports i JOIN users u ON u.id = i.user_id
         WHERE i.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((user_id, username, is_admin)) = owner else {
        return Ok(None);
    };
    Ok(Some(User {
        id: user_id,
        username,
        is_admin,
    }))
}

/// A pending track: its position, title, artists, album, ISRC and length.
type Pending = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

/// The import's tracks still pending, in order.
async fn pending(pool: &SqlitePool, id: i64) -> anyhow::Result<Vec<Pending>> {
    let pending: Vec<Pending> = sqlx::query_as(
        "SELECT position, title, artist, album, isrc, length_ms FROM playlist_import_tracks
         WHERE import_id = ? AND state = 'pending' ORDER BY position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(pending)
}

/// Marks the import done, with what became of its tracks.
async fn finish(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    let done = summary(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("import {id} vanished"))?;
    let mut said = vec![format!("{} asked for", done.asked)];
    if done.already > 0 {
        said.push(format!("{} asked for already", done.already));
    }
    if done.unmatched > 0 {
        said.push(format!("{} not found on MusicBrainz", done.unmatched));
    }
    sqlx::query(
        "UPDATE playlist_imports SET state = 'done', message = ?, finished_at = ? WHERE id = ?",
    )
    .bind(said.join(", "))
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// What became of one track: its state, why it wasn't asked for, its request, and whether it
/// was approved at once and is to be fetched.
pub(super) async fn ask(
    pool: &SqlitePool,
    mb: &musicbrainz::Client,
    user: &User,
    track: &Track,
) -> anyhow::Result<(&'static str, Option<String>, Option<i64>, bool)> {
    let mut tries = 0;
    let recording = loop {
        match recording_for(mb, track).await {
            Ok(found) => break found,
            Err(err) => {
                tries += 1;
                if tries == TRIES {
                    return Ok((
                        "unmatched",
                        Some(format!("MusicBrainz didn't answer: {err}")),
                        None,
                        false,
                    ));
                }
                tracing::info!(error = %err, "MusicBrainz didn't answer; trying again in a minute");
                tokio::time::sleep(RETRY).await;
            }
        }
    };
    let recording = match recording {
        Ok(id) => id,
        Err(why) => return Ok(("unmatched", Some(why), None, false)),
    };
    let new = NewRequest {
        kind: "song".into(),
        mb_recording_id: Some(recording),
        ..NewRequest::default()
    };
    Ok(
        match requests::create(pool, None, Some(mb), user, new).await? {
            Ok(view) => {
                let fetch = view.state == "approved" && view.decided_by.is_some();
                ("asked", None, Some(view.id), fetch)
            }
            Err(Refused::Already(id)) => ("already", None, Some(id), false),
            Err(refused) => ("unmatched", Some(refused.to_string()), None, false),
        },
    )
}
