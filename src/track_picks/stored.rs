//! Users' languages and their choices per series, as the database keeps them.

use super::*;

/// The languages in a stored JSON array, leaving out any that no longer read as one.
fn languages(json: &str) -> Vec<Language> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|code| Language::parse(code))
        .collect()
}

/// The codes of `languages` as a JSON array.
fn codes(languages: &[Language]) -> String {
    let codes: Vec<String> = languages.iter().map(Language::code).collect();
    serde_json::Value::from(codes).to_string()
}

/// A user's languages; the defaults (none, and subtitles when the audio is foreign) for one who
/// hasn't set any.
pub async fn preferences(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Preferences> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT audio_languages, subtitle_languages, subtitle_mode FROM user_languages
         WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => Preferences::default(),
        Some((audio, subtitles, mode)) => Preferences {
            audio: languages(&audio),
            subtitles: languages(&subtitles),
            mode: SubtitleMode::parse(&mode).unwrap_or(SubtitleMode::Foreign),
        },
    })
}

/// A kind of title with languages of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movies,
    Shows,
    Anime,
}

impl Kind {
    /// Every kind as the API names it.
    pub const NAMES: [&str; 3] = ["movies", "shows", "anime"];

    pub fn parse(name: &str) -> Option<Kind> {
        Some(match name {
            "movies" => Kind::Movies,
            "shows" => Kind::Shows,
            "anime" => Kind::Anime,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Movies => "movies",
            Kind::Shows => "shows",
            Kind::Anime => "anime",
        }
    }
}

/// A user's own languages for `kind`, or None when that kind follows their languages.
pub async fn kind_preferences(
    pool: &SqlitePool,
    user_id: i64,
    kind: Kind,
) -> sqlx::Result<Option<Preferences>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT audio_languages, subtitle_languages, subtitle_mode FROM user_kind_languages
         WHERE user_id = ? AND kind = ?",
    )
    .bind(user_id)
    .bind(kind.name())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(audio, subtitles, mode)| Preferences {
        audio: languages(&audio),
        subtitles: languages(&subtitles),
        mode: SubtitleMode::parse(&mode).unwrap_or(SubtitleMode::Foreign),
    }))
}

/// Keeps a user's own languages for `kind`, replacing what it had.
pub async fn set_kind_preferences(
    pool: &SqlitePool,
    user_id: i64,
    kind: Kind,
    preferences: &Preferences,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO user_kind_languages
             (user_id, kind, audio_languages, subtitle_languages, subtitle_mode, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, kind) DO UPDATE SET audio_languages = excluded.audio_languages,
             subtitle_languages = excluded.subtitle_languages,
             subtitle_mode = excluded.subtitle_mode, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(kind.name())
    .bind(codes(&preferences.audio))
    .bind(codes(&preferences.subtitles))
    .bind(preferences.mode.name())
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Lets `kind` follow the user's languages again; whether it had its own.
pub async fn clear_kind_preferences(
    pool: &SqlitePool,
    user_id: i64,
    kind: Kind,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM user_kind_languages WHERE user_id = ? AND kind = ?")
            .bind(user_id)
            .bind(kind.name())
            .execute(pool)
            .await?
            .rows_affected()
            > 0,
    )
}

/// The kind file `file_id` is of: a movie's, else a series' (anime when it's wanted as anime or
/// in a library with the anime category, as the series lists decide it); None for neither.
async fn kind_of_file(pool: &SqlitePool, file_id: i64) -> sqlx::Result<Option<Kind>> {
    let row: Option<(Option<i64>, Option<i64>, bool)> = sqlx::query_as(
        "SELECT f.movie_id, f.series_id,
             f.series_id IS NOT NULL AND (l.category IS 'anime' OR EXISTS (
                 SELECT 1 FROM wanted_series w
                 WHERE w.series_id = f.series_id AND w.series_type = 'anime'))
         FROM media_files f JOIN libraries l ON l.id = f.library_id WHERE f.id = ?",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.and_then(|(movie, series, anime)| match (movie, series) {
            (Some(_), _) => Some(Kind::Movies),
            (None, Some(_)) if anime => Some(Kind::Anime),
            (None, Some(_)) => Some(Kind::Shows),
            _ => None,
        }),
    )
}

/// Keeps a user's languages, replacing what they had.
pub async fn set_preferences(
    pool: &SqlitePool,
    user_id: i64,
    preferences: &Preferences,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO user_languages
             (user_id, audio_languages, subtitle_languages, subtitle_mode, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET audio_languages = excluded.audio_languages,
             subtitle_languages = excluded.subtitle_languages,
             subtitle_mode = excluded.subtitle_mode, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(codes(&preferences.audio))
    .bind(codes(&preferences.subtitles))
    .bind(preferences.mode.name())
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct ChoiceRow {
    audio_language: Option<String>,
    subtitles: String,
    subtitle_language: Option<String>,
    subtitle_forced: bool,
    subtitle_hearing_impaired: bool,
}

/// What a user last chose by hand in a series; nothing for one they haven't.
pub async fn series_choice(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
) -> sqlx::Result<SeriesChoice> {
    let row: Option<ChoiceRow> = sqlx::query_as(
        "SELECT audio_language, subtitles, subtitle_language, subtitle_forced,
             subtitle_hearing_impaired
         FROM series_track_choices WHERE user_id = ? AND series_id = ?",
    )
    .bind(user_id)
    .bind(series_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(SeriesChoice::default());
    };
    let subtitles = match row.subtitles.as_str() {
        "off" => Some(SubtitleChoice::Off),
        "on" => row
            .subtitle_language
            .as_deref()
            .and_then(Language::parse)
            .map(|language| SubtitleChoice::On {
                language,
                forced: row.subtitle_forced,
                hearing_impaired: row.subtitle_hearing_impaired,
            }),
        _ => None,
    };
    Ok(SeriesChoice {
        audio: row.audio_language.as_deref().and_then(Language::parse),
        subtitles,
    })
}

/// Keeps what a user chose by hand in a series, replacing their last choice there.
pub async fn set_series_choice(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
    choice: &SeriesChoice,
) -> sqlx::Result<()> {
    let (subtitles, language, forced, hearing_impaired) = match &choice.subtitles {
        None => ("unknown", None, false, false),
        Some(SubtitleChoice::Off) => ("off", None, false, false),
        Some(SubtitleChoice::On {
            language,
            forced,
            hearing_impaired,
        }) => ("on", Some(language.code()), *forced, *hearing_impaired),
    };
    sqlx::query(
        "INSERT INTO series_track_choices (user_id, series_id, audio_language, subtitles,
             subtitle_language, subtitle_forced, subtitle_hearing_impaired, chosen_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, series_id) DO UPDATE SET audio_language = excluded.audio_language,
             subtitles = excluded.subtitles, subtitle_language = excluded.subtitle_language,
             subtitle_forced = excluded.subtitle_forced,
             subtitle_hearing_impaired = excluded.subtitle_hearing_impaired,
             chosen_at = excluded.chosen_at",
    )
    .bind(user_id)
    .bind(series_id)
    .bind(choice.audio.as_ref().map(Language::code))
    .bind(subtitles)
    .bind(language)
    .bind(forced)
    .bind(hearing_impaired)
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// The tracks to play a file with for a user, from its probe and the subtitles beside it.
pub async fn for_file(
    pool: &SqlitePool,
    user_id: i64,
    file_id: i64,
    media: Option<&MediaInfo>,
    sidecars: &[Sidecar],
) -> sqlx::Result<Pick> {
    // The file's kind's own languages, else the user's.
    let own = match kind_of_file(pool, file_id).await? {
        Some(kind) => kind_preferences(pool, user_id, kind).await?,
        None => None,
    };
    let preferences = match own {
        Some(own) => own,
        None => preferences(pool, user_id).await?,
    };
    let series: Option<Option<i64>> =
        sqlx::query_scalar("SELECT series_id FROM media_files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(pool)
            .await?;
    let choice = match series.flatten() {
        Some(series_id) => series_choice(pool, user_id, series_id).await?,
        None => SeriesChoice::default(),
    };
    Ok(pick(
        &preferences,
        &choice,
        media,
        &subtitle_tracks(media, sidecars),
    ))
}
