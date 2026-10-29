//! What a user may see: the libraries an admin lets them see, and the highest content rating.
//! Every query that lists or opens a title, file or track takes its condition from here, so
//! the rule is written once: a title outside a user's access is as if it weren't there.
//!
//! Admins, and users nobody has restricted, see everything. With a highest rating, a movie is
//! seen only when its rating is known and no higher; an episode goes by its series'; a title
//! without a TMDB match, or whose ratings Melete can't read, is hidden. Music has no ratings, so
//! only its libraries count.

use sqlx::SqlitePool;

/// The age a missing or unreadable rating counts as: above any limit.
const UNRATED: i64 = 99;

/// What a user may see.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Access {
    /// The libraries they may see; None for every library.
    libraries: Option<Vec<i64>>,
    /// The highest age a title may be rated for; None for no limit.
    max_age: Option<i64>,
}

/// A user's access as an admin set it, before it's taken into account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restriction {
    /// The libraries they may see by id; None for every library, those added later too.
    pub libraries: Option<Vec<i64>>,
    /// The highest age a title may be rated for; None for no limit.
    pub max_age: Option<i64>,
}

impl Access {
    /// Everything: an admin's, or a user's nobody has restricted.
    pub fn everything() -> Access {
        Access::default()
    }

    /// Only these libraries, and titles rated up to `max_age`.
    pub fn limited(libraries: Option<Vec<i64>>, max_age: Option<i64>) -> Access {
        Access { libraries, max_age }
    }

    /// The access of a user: everything for an admin or a user without a restriction.
    pub async fn of(pool: &SqlitePool, user_id: i64, is_admin: bool) -> sqlx::Result<Access> {
        if is_admin {
            return Ok(Access::everything());
        }
        Ok(match restriction(pool, user_id).await? {
            None => Access::everything(),
            Some(r) => Access::limited(r.libraries, r.max_age),
        })
    }

    /// Whether nothing is hidden.
    pub fn is_everything(&self) -> bool {
        self.libraries.is_none() && self.max_age.is_none()
    }

    /// Whether only some libraries may be seen.
    pub fn limits_libraries(&self) -> bool {
        self.libraries.is_some()
    }

    /// Whether there's a highest rating, which hides every title whose rating isn't known.
    pub fn limits_rating(&self) -> bool {
        self.max_age.is_some()
    }

    /// SQL: whether the library id in `column` may be seen. `column` is code, never input.
    pub fn library_sql(&self, column: &str) -> String {
        match &self.libraries {
            None => "1".into(),
            Some(ids) => {
                let ids: Vec<String> = ids.iter().map(i64::to_string).collect();
                format!("({column} IN ({}))", ids.join(", "))
            }
        }
    }

    /// SQL: whether a title whose TMDB id is in `tmdb_id` (a movie's, or with `tv` a series')
    /// is rated low enough. `tmdb_id` is code, never input.
    pub fn rating_sql(&self, tv: bool, tmdb_id: &str) -> String {
        match self.max_age {
            None => "1".into(),
            Some(max) => format!(
                "(COALESCE((SELECT r.age FROM tmdb_ratings r WHERE r.tv = {} AND r.tmdb_id = {tmdb_id}),
                     {UNRATED}) <= {max})",
                i64::from(tv)
            ),
        }
    }

    /// SQL: whether the movie row `movie` (an alias of `movies`) may be seen.
    pub fn movie_sql(&self, movie: &str) -> String {
        format!(
            "({} AND {})",
            self.library_sql(&format!("{movie}.library_id")),
            self.rating_sql(false, &format!("{movie}.match_tmdb_id"))
        )
    }

    /// SQL: whether the series row `series` (an alias of `series`) may be seen.
    pub fn series_sql(&self, series: &str) -> String {
        format!(
            "({} AND {})",
            self.library_sql(&format!("{series}.library_id")),
            self.rating_sql(true, &format!("{series}.match_tmdb_id"))
        )
    }

    /// SQL: whether the file row `file` (an alias of `media_files`) may be seen: its library,
    /// and its movie's or series' rating.
    pub fn file_sql(&self, file: &str) -> String {
        if self.max_age.is_none() {
            return self.library_sql(&format!("{file}.library_id"));
        }
        format!(
            "({} AND (EXISTS (SELECT 1 FROM movies access_m
                   WHERE access_m.id = {file}.movie_id AND {})
               OR EXISTS (SELECT 1 FROM series access_s
                   WHERE access_s.id = {file}.series_id AND {})))",
            self.library_sql(&format!("{file}.library_id")),
            self.rating_sql(false, "access_m.match_tmdb_id"),
            self.rating_sql(true, "access_s.match_tmdb_id"),
        )
    }

    /// SQL: whether the extra row `extra` (an alias of `media_extras`) may be seen: as its
    /// movie or series may.
    pub fn extra_sql(&self, extra: &str) -> String {
        if self.is_everything() {
            return "1".into();
        }
        format!(
            "(EXISTS (SELECT 1 FROM movies access_m WHERE access_m.id = {extra}.movie_id AND {})
              OR EXISTS (SELECT 1 FROM series access_s
                  WHERE access_s.id = {extra}.series_id AND {}))",
            self.movie_sql("access_m"),
            self.series_sql("access_s"),
        )
    }

    /// SQL: whether the album row `album` (an alias of `music_albums`) may be seen. Music has no
    /// ratings, so only its library counts.
    pub fn album_sql(&self, album: &str) -> String {
        self.library_sql(&format!("{album}.library_id"))
    }

    /// SQL: whether the track row `track` (an alias of `music_tracks`) may be seen.
    pub fn track_sql(&self, track: &str) -> String {
        self.library_sql(&format!("{track}.library_id"))
    }

    /// Whether the user may see every music library, as Aoide's sync and what's shared between
    /// listeners need: their payloads aren't read, so they can't be filtered.
    pub async fn sees_all_music(&self, pool: &SqlitePool) -> sqlx::Result<bool> {
        let Some(allowed) = &self.libraries else {
            return Ok(true);
        };
        let music: Vec<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE kind = 'music'")
            .fetch_all(pool)
            .await?;
        Ok(music.iter().all(|id| allowed.contains(id)))
    }

    /// Whether the album with this id is there for the user to see.
    pub async fn sees_album(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "music_albums", &self.album_sql("x"), id)
            .await
    }

    /// Whether the track with this id is there for the user to see.
    pub async fn sees_track(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "music_tracks", &self.track_sql("x"), id)
            .await
    }

    /// Whether the movie with this id is there for the user to see.
    pub async fn sees_movie(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "movies", &self.movie_sql("x"), id).await
    }

    /// Whether the series with this id is there for the user to see.
    pub async fn sees_series(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "series", &self.series_sql("x"), id).await
    }

    /// Whether the extra with this id is there for the user to see.
    pub async fn sees_extra(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "media_extras", &self.extra_sql("x"), id)
            .await
    }

    /// Whether the file with this id is there for the user to see.
    pub async fn sees_file(&self, pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
        self.sees(pool, "media_files", &self.file_sql("x"), id)
            .await
    }

    /// Whether a movie (or with `tv`, a series) known by its TMDB id is rated low enough, by the
    /// rating Melete has for it, in the library or not: always without a highest rating, never
    /// with one when its rating isn't known.
    pub async fn rates_tmdb(
        &self,
        pool: &SqlitePool,
        tv: bool,
        tmdb_id: i64,
    ) -> sqlx::Result<bool> {
        if !self.limits_rating() {
            return Ok(true);
        }
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {}",
            self.rating_sql(tv, "?")
        )))
        .bind(tmdb_id)
        .fetch_one(pool)
        .await
    }

    /// Whether a movie (or with `tv`, a series) known by its TMDB id may be shown, as a search
    /// result, say: one in the library when the user may see it there, one that isn't when it's
    /// rated low enough ([`Access::rates_tmdb`]; its rating fetched first by the caller).
    pub async fn shows_tmdb(
        &self,
        pool: &SqlitePool,
        tv: bool,
        tmdb_id: i64,
    ) -> sqlx::Result<bool> {
        if self.is_everything() {
            return Ok(true);
        }
        let (table, seen) = if tv {
            ("series", self.series_sql("x"))
        } else {
            ("movies", self.movie_sql("x"))
        };
        let (present, visible): (bool, bool) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) > 0, coalesce(max({seen}), 0) FROM {table} x
             WHERE x.match_tmdb_id = ? AND x.missing_since IS NULL"
        )))
        .bind(tmdb_id)
        .fetch_one(pool)
        .await?;
        if present {
            return Ok(visible);
        }
        self.rates_tmdb(pool, tv, tmdb_id).await
    }

    async fn sees(
        &self,
        pool: &SqlitePool,
        table: &str,
        condition: &str,
        id: i64,
    ) -> sqlx::Result<bool> {
        let found: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT x.id FROM {table} x WHERE x.id = ? AND {condition}"
        )))
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(found.is_some())
    }
}

/// What an admin set for a user; None when nothing is restricted.
pub async fn restriction(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<Restriction>> {
    let row: Option<(bool, Option<i64>)> =
        sqlx::query_as("SELECT all_libraries, max_age FROM user_access WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let Some((all, max_age)) = row else {
        return Ok(None);
    };
    let libraries = if all {
        None
    } else {
        Some(
            sqlx::query_scalar(
                "SELECT library_id FROM user_libraries WHERE user_id = ? ORDER BY library_id",
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?,
        )
    };
    if libraries.is_none() && max_age.is_none() {
        return Ok(None);
    }
    Ok(Some(Restriction { libraries, max_age }))
}

/// Sets what a user may see, replacing what was set; nothing restricted forgets it.
pub async fn set_restriction(
    pool: &SqlitePool,
    user_id: i64,
    restriction: Option<&Restriction>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_libraries WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM user_access WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    if let Some(r) = restriction.filter(|r| r.libraries.is_some() || r.max_age.is_some()) {
        sqlx::query(
            "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(r.libraries.is_none())
        .bind(r.max_age)
        .bind(crate::db::unix_now())
        .execute(&mut *tx)
        .await?;
        for library in r.libraries.iter().flatten() {
            sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, ?)")
                .bind(user_id)
                .bind(library)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

#[cfg(test)]
mod tests;
