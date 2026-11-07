//! A rule set compiled to one SQL statement over the library and the listener's own history,
//! the way the apps compile it over their track cache: the same columns as a device fills them
//! from the music API, the same play and skip lines, the same order and tie-break.

use std::collections::HashSet;

use sqlx::SqlitePool;

use super::rule::{Field, Kind, Operator, Rule, Value};
use super::{Direction, Interval, Match, Rules, SmartError, Sort};

/// Four minutes of listening is a play however long the track is.
const PLAY_CEILING_MS: i64 = 240_000;
/// A play is at least half the track (or the ceiling); a skip under a fifth of it.
const PLAY_DIVISOR: i64 = 2;
const SKIP_DIVISOR: i64 = 5;

/// Whose songs: the listener, whose likes, flags and listens rules about them read, and their
/// offset from UTC in minutes, for months and years counted on their calendar.
#[derive(Debug, Clone, Copy)]
pub struct Listener {
    /// The listener.
    pub user_id: i64,
    /// Minutes east of UTC.
    pub offset_minutes: i64,
}

/// A bound value.
#[derive(Debug, Clone, PartialEq)]
enum Arg {
    Int(i64),
    Text(String),
}

/// Present tracks as a device's track cache holds them, from the music API: artist is the
/// credited artists joined with ", " (else the album's artist), year the track's else the
/// album's, duration whole milliseconds.
const TRACK: &str = "SELECT t.id, t.title,
         coalesce((SELECT group_concat(ta.name, ', ' ORDER BY ta.position)
             FROM music_track_artists ta WHERE ta.track_id = t.id), a.artist_name) AS artist,
         a.title AS album, a.artist_name AS album_artist, t.genres,
         coalesce(t.year, a.year) AS year,
         CAST(coalesce(t.duration_seconds, (SELECT p.duration_seconds FROM music_track_probes p
             WHERE p.track_id = t.id AND p.size = t.size AND p.modified_at = t.modified_at))
             * 1000 AS INTEGER) AS duration_ms
     FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
     WHERE t.missing_since IS NULL";

/// Whether listen `p` of track `d` counts as a play: it reached the end, or it ended, wasn't
/// skipped, and ran half the track or four minutes, whichever is less.
fn is_play() -> String {
    format!(
        "(p.completed = 1 OR (p.skipped = 0 AND p.ended_at IS NOT NULL
         AND coalesce(p.position_ms, 0) >= CASE WHEN d.duration_ms IS NULL OR d.duration_ms <= 0
             THEN {PLAY_CEILING_MS} ELSE min(d.duration_ms / {PLAY_DIVISOR}, {PLAY_CEILING_MS}) END))"
    )
}

/// Whether it counts as a skip: it didn't reach the end, and was skipped, or ended inside the
/// first fifth (of four minutes, when the length isn't known). Never also a play.
fn is_skip() -> String {
    format!(
        "(p.completed = 0 AND (p.skipped = 1 OR (p.ended_at IS NOT NULL
         AND p.position_ms < CASE WHEN d.duration_ms IS NULL OR d.duration_ms <= 0
             THEN {} ELSE d.duration_ms / {SKIP_DIVISOR} END)))",
        PLAY_CEILING_MS / SKIP_DIVISOR
    )
}

/// A rule set ready to run.
pub struct Compiled {
    sql: String,
    args: Vec<Arg>,
}

impl Compiled {
    /// Compiles `rules` as of `now` (epoch milliseconds) for `listener`.
    ///
    /// # Errors
    ///
    /// [`SmartError::FieldNotSortable`] for a sort on genre.
    pub fn new(rules: &Rules, now: i64, listener: Listener) -> Result<Self, SmartError> {
        let mut predicates = Vec::new();
        let mut args = Vec::new();
        for rule in &rules.rules {
            let (fragment, bound) = predicate(rule, now, listener.offset_minutes);
            predicates.push(format!("({fragment})"));
            args.extend(bound);
        }
        let filter = if predicates.is_empty() {
            "1".to_owned()
        } else {
            predicates.join(match rules.match_mode {
                Match::All => " AND ",
                Match::Any => " OR ",
            })
        };
        let mut tail = String::new();
        if let Some(limit) = rules.limit {
            tail = "LIMIT ?".into();
            args.push(Arg::Int(limit));
        }
        let sql = format!(
            "WITH liked(id) AS (SELECT value FROM json_each(?)),
                  hidden(id) AS (SELECT value FROM json_each(?)),
                  track AS ({TRACK}),
                  history AS (
                      SELECT p.track_id,
                          sum(CASE WHEN {play} THEN 1 ELSE 0 END) AS play_count,
                          sum(CASE WHEN {skip} THEN 1 ELSE 0 END) AS skip_count,
                          max(CASE WHEN {play} THEN p.started_at END) AS last_played,
                          min(CASE WHEN {play} THEN p.started_at END) AS first_played
                      FROM music_plays p LEFT JOIN track d ON d.id = p.track_id
                      WHERE p.user_id = ? AND p.track_id IS NOT NULL
                      GROUP BY p.track_id)
             SELECT t.id FROM track t LEFT JOIN history h ON h.track_id = t.id
             WHERE t.id NOT IN (SELECT id FROM hidden) AND ({filter})
             ORDER BY {order} {tail}",
            play = is_play(),
            skip = is_skip(),
            order = order(rules.sort)?,
        );
        Ok(Self { sql, args })
    }

    /// The matching track ids, in the rule set's order. `liked` and `hidden` are the
    /// listener's liked and Not interested tracks.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub async fn run(
        &self,
        pool: &SqlitePool,
        user_id: i64,
        liked: &HashSet<i64>,
        hidden: &HashSet<i64>,
    ) -> sqlx::Result<Vec<i64>> {
        let ids = |set: &HashSet<i64>| serde_json::to_string(set).unwrap_or_else(|_| "[]".into());
        let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(self.sql.clone()))
            .bind(ids(liked))
            .bind(ids(hidden))
            .bind(user_id);
        for arg in &self.args {
            query = match arg {
                Arg::Int(n) => query.bind(*n),
                Arg::Text(text) => query.bind(text.clone()),
            };
        }
        query.fetch_all(pool).await
    }
}

/// The songs `rules` give `listener` now, in order: the library's present tracks but those
/// the listener marked Not interested.
///
/// # Errors
///
/// [`EvaluateError::Rules`] when the rule set can't be run (a sort on genre), else a database
/// error.
pub async fn evaluate(
    pool: &SqlitePool,
    rules: &Rules,
    now: i64,
    listener: Listener,
) -> Result<Vec<i64>, EvaluateError> {
    let compiled = Compiled::new(rules, now, listener).map_err(EvaluateError::Rules)?;
    let liked: HashSet<i64> =
        crate::aoide::likes::liked(pool, crate::aoide::likes::Kind::Track, listener.user_id)
            .await?
            .into_iter()
            .filter_map(|(track, _)| track)
            .collect();
    let hidden: HashSet<i64> = crate::aoide::listening::not_interested(pool, listener.user_id)
        .await?
        .iter()
        .filter_map(|id| id.parse().ok())
        .collect();
    Ok(compiled
        .run(pool, listener.user_id, &liked, &hidden)
        .await?)
}

/// Why a rule set couldn't be worked out.
#[derive(Debug)]
pub enum EvaluateError {
    /// The rule set can't be run.
    Rules(SmartError),
    /// The database failed.
    Database(sqlx::Error),
}

impl From<sqlx::Error> for EvaluateError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err)
    }
}

fn column(field: Field) -> &'static str {
    match field {
        Field::Title => "t.title",
        Field::Artist => "t.artist",
        Field::Album => "t.album",
        // A comparison with NULL is NULL: "album artist is not X" would drop the tracks with
        // none, the ones most plainly meeting it.
        Field::AlbumArtist => "IFNULL(t.album_artist, '')",
        Field::Genre => "t.genres",
        Field::Year => "t.year",
        Field::DurationMs => "t.duration_ms",
        Field::PlayCount => "coalesce(h.play_count, 0)",
        Field::SkipCount => "coalesce(h.skip_count, 0)",
        // Not coalesced: never played isn't played at the epoch.
        Field::LastPlayed => "h.last_played",
        Field::FirstPlayed => "h.first_played",
        Field::Liked => "(t.id IN (SELECT id FROM liked))",
    }
}

fn predicate(rule: &Rule, now: i64, offset_minutes: i64) -> (String, Vec<Arg>) {
    let column = column(rule.field);
    let bound = match (rule.field.kind(), &rule.value) {
        (Kind::Text, Value::Text(text)) if rule.field == Field::Genre => {
            return genre(rule.op, text);
        }
        (Kind::Text, Value::Text(text)) => return text_predicate(rule.op, column, text),
        (Kind::Number, Value::Number(bound)) => *bound,
        (Kind::Date, Value::Text(written)) => match Interval::parse(written) {
            Ok(span) => span.ago(now, offset_minutes),
            Err(_) => return ("0".into(), Vec::new()),
        },
        (Kind::Date, Value::Number(at)) => *at,
        (Kind::Flag, Value::Flag(flag)) => i64::from(*flag),
        // Unreachable past `Rule::new`: a false predicate rather than a crash.
        _ => return ("0".into(), Vec::new()),
    };
    match comparison(rule.field.kind(), rule.op, column) {
        Some(fragment) => (fragment, vec![Arg::Int(bound)]),
        None => ("0".into(), Vec::new()),
    }
}

/// A number, date or flag against one bound; none for a pair a rule can't hold.
fn comparison(kind: Kind, op: Operator, column: &str) -> Option<String> {
    Some(match (kind, op) {
        (Kind::Number | Kind::Flag, Operator::Is) => format!("{column} = ?"),
        // NOT (NULL = 1999) is NULL: "year is not 1999" keeps the years nobody knows.
        (Kind::Number, Operator::IsNot) => format!("({column} IS NULL OR {column} <> ?)"),
        (Kind::Number, Operator::GreaterThan) => format!("{column} > ?"),
        (Kind::Number, Operator::LessThan) => format!("{column} < ?"),
        // NULL fails both: before and after say a play happened.
        (Kind::Date, Operator::Before) => format!("{column} < ?"),
        (Kind::Date, Operator::After) => format!("{column} > ?"),
        (Kind::Date, Operator::InTheLast) => format!("{column} >= ?"),
        // "Not played in the last month" includes never played.
        (Kind::Date, Operator::NotInTheLast) => format!("({column} IS NULL OR {column} < ?)"),
        (Kind::Flag, Operator::IsNot) => format!("{column} <> ?"),
        _ => return None,
    })
}

/// NOCASE, so "artist is beatles" finds The Beatles; ASCII only, as SQLite's is.
fn text_predicate(op: Operator, column: &str, text: &str) -> (String, Vec<Arg>) {
    let (sql, arg) = match op {
        Operator::Is => (format!("{column} = ? COLLATE NOCASE"), text.to_owned()),
        Operator::IsNot => (format!("{column} <> ? COLLATE NOCASE"), text.to_owned()),
        Operator::Contains => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            wildcarded(text, true),
        ),
        Operator::DoesNotContain => (
            format!("{column} NOT LIKE ? ESCAPE '\\'"),
            wildcarded(text, true),
        ),
        Operator::BeginsWith => (
            format!("{column} LIKE ? ESCAPE '\\'"),
            wildcarded(text, false),
        ),
        _ => return ("0".into(), Vec::new()),
    };
    (sql, vec![Arg::Text(arg)])
}

/// Genres are a JSON array, so every operator asks about some genre, and the negative ones
/// stand outside the EXISTS: inside, "is not Jazz" would match the jazz too.
fn genre(op: Operator, text: &str) -> (String, Vec<Arg>) {
    let (test, arg) = match op {
        Operator::Is | Operator::IsNot => ("g.value = ? COLLATE NOCASE", text.to_owned()),
        Operator::Contains | Operator::DoesNotContain => {
            ("g.value LIKE ? ESCAPE '\\'", wildcarded(text, true))
        }
        Operator::BeginsWith => ("g.value LIKE ? ESCAPE '\\'", wildcarded(text, false)),
        _ => return ("0".into(), Vec::new()),
    };
    let not = if matches!(op, Operator::IsNot | Operator::DoesNotContain) {
        "NOT "
    } else {
        ""
    };
    (
        format!("{not}EXISTS (SELECT 1 FROM json_each(t.genres) AS g WHERE {test})"),
        vec![Arg::Text(arg)],
    )
}

/// A LIKE pattern whose only wildcards are the ones added here: "100%" mustn't match every
/// title with 100 in it.
fn wildcarded(text: &str, prefix: bool) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{}{escaped}%", if prefix { "%" } else { "" })
}

/// Always an order, the track id (as text, as a device's cache holds it) breaking ties, so a
/// limit takes the same songs every time.
fn order(sort: Option<Sort>) -> Result<String, SmartError> {
    let Some(sort) = sort else {
        return Ok("t.title COLLATE NOCASE, CAST(t.id AS TEXT)".into());
    };
    if sort.field == Field::Genre {
        return Err(SmartError::FieldNotSortable(Field::Genre));
    }
    let expression = if sort.field.kind() == Kind::Text {
        format!("{} COLLATE NOCASE", column(sort.field))
    } else {
        column(sort.field).to_owned()
    };
    let dir = match sort.dir {
        Direction::Asc => "ASC",
        Direction::Desc => "DESC",
    };
    Ok(format!("{expression} {dir}, CAST(t.id AS TEXT)"))
}
