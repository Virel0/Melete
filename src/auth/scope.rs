//! What an API key is for, and the lookup that tells a request's token apart: whose it is,
//! which one, and what it may reach.

use super::*;

/// What a token may reach. Sessions are always [`Scope::Full`]; an API key is given one when
/// it's made, and keys made before scopes existed are full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Browsing, and reading one's own data: GET and HEAD on routes for any signed-in user.
    Read,
    /// What [`Scope::Read`] reaches, plus playing (stream links) and reporting progress.
    Playback,
    /// Aoide's routes alone, reading and writing.
    MusicSync,
    /// Everything the token's user can do, admin routes included for an admin.
    Full,
}

impl Scope {
    /// The scope as stored and sent: "read", "playback", "music-sync" or "full".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Playback => "playback",
            Self::MusicSync => "music-sync",
            Self::Full => "full",
        }
    }

    /// The scope named by its stored name; None for anything else.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "read" => Self::Read,
            "playback" => Self::Playback,
            "music-sync" => Self::MusicSync,
            "full" => Self::Full,
            _ => return None,
        })
    }

    /// Whether a token of this scope reaches a route that needs `needed`.
    pub fn reaches(self, needed: Scope) -> bool {
        match self {
            Self::Full => true,
            Self::Playback => matches!(needed, Self::Read | Self::Playback),
            Self::Read | Self::MusicSync => needed == self,
        }
    }
}

/// Who a token belongs to, which token it is, and what it may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// Its user.
    pub user: User,
    /// The token's row id.
    pub token_id: i64,
    /// What it may reach.
    pub scope: Scope,
}

/// Issues a token of `scope` and returns its id and the token itself, which is never stored.
/// Only an API key may be less than full.
pub async fn issue_scoped(
    pool: &SqlitePool,
    user_id: i64,
    kind: TokenKind,
    name: &str,
    scope: Scope,
) -> anyhow::Result<(i64, String)> {
    // Sessions that went unused long enough to have expired leave the device list.
    sqlx::query(
        "DELETE FROM tokens WHERE user_id = ? AND kind = 'session' AND coalesce(last_used_at, created_at) < ?",
    )
    .bind(user_id)
    .bind(unix_now() - SESSION_IDLE)
    .execute(pool)
    .await?;
    let token = generate_token()?;
    let id = sqlx::query_scalar(
        "INSERT INTO tokens (user_id, kind, name, token_hash, created_at, scope)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(kind.as_str())
    .bind(name)
    .bind(token_hash(&token))
    .bind(unix_now())
    .bind(scope.as_str())
    .fetch_one(pool)
    .await
    .context("storing token")?;
    Ok((id, token))
}

/// Whose `token` is, and notes it as used; None for a token that isn't valid, or a session
/// unused too long, which is signed out.
pub async fn token_holder(pool: &SqlitePool, token: &str) -> anyhow::Result<Option<Holder>> {
    if !token.starts_with(TOKEN_PREFIX) {
        return Ok(None);
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        username: String,
        is_admin: bool,
        token_id: i64,
        last_used_at: Option<i64>,
        kind: String,
        created_at: i64,
        scope: String,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT u.id, u.username, u.is_admin, t.id AS token_id, t.last_used_at, t.kind, t.created_at,
             t.scope
         FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ?",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    // The database allows only these; anything else is refused rather than guessed at.
    let Some(scope) = Scope::parse(&row.scope) else {
        return Ok(None);
    };
    let now = unix_now();
    let last_used = row.last_used_at;
    // A device unused for 90 days signs in again; its stream links go with it.
    if row.kind == "session" && now - last_used.unwrap_or(row.created_at) > SESSION_IDLE {
        sqlx::query("DELETE FROM tokens WHERE id = ?")
            .bind(row.token_id)
            .execute(pool)
            .await?;
        return Ok(None);
    }
    if last_used.is_none_or(|t| now - t >= LAST_USED_RESOLUTION) {
        sqlx::query("UPDATE tokens SET last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(row.token_id)
            .execute(pool)
            .await?;
    }
    Ok(Some(Holder {
        user: User {
            id: row.id,
            username: row.username,
            is_admin: row.is_admin,
        },
        token_id: row.token_id,
        scope,
    }))
}
