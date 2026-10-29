//! Lyrics translated through LibreTranslate, once per song and language, and kept.

use super::*;

/// Whether a language code is one to ask LibreTranslate for: "en", "pt-BR", "zh-Hans".
pub fn language_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let first = parts.next().unwrap_or_default();
    (2..=3).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=4).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
        && code.split('-').count() <= 2
}

/// Each of the lyrics' lines (as written) in `language`: translated through LibreTranslate
/// once per song and language, and kept; translated afresh when the lyrics change. Empty lines
/// aren't sent.
pub async fn translation(
    pool: &SqlitePool,
    client: &crate::libretranslate::Client,
    track_id: i64,
    lyrics: &Lyrics,
    language: &str,
) -> Result<HashMap<String, String>, Error> {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(lyrics.text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let kept: Option<String> = sqlx::query_scalar(
        "SELECT lines FROM music_lyrics_translations
         WHERE track_id = ? AND language = ? AND lyrics_hash = ?",
    )
    .bind(track_id)
    .bind(language)
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    if let Some(lines) = kept.and_then(|k| serde_json::from_str(&k).ok()) {
        return Ok(lines);
    }
    let mut wanted: Vec<String> = Vec::new();
    for line in lyrics.lines() {
        if !line.text.trim().is_empty() && !wanted.contains(&line.text) {
            wanted.push(line.text);
        }
    }
    let translated = client
        .translate(&wanted, language)
        .await
        .map_err(Error::Translation)?;
    let lines: HashMap<String, String> = wanted.into_iter().zip(translated).collect();
    sqlx::query(
        "INSERT INTO music_lyrics_translations (track_id, language, lyrics_hash, lines,
             translated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (track_id, language) DO UPDATE SET lyrics_hash = excluded.lyrics_hash,
             lines = excluded.lines, translated_at = excluded.translated_at",
    )
    .bind(track_id)
    .bind(language)
    .bind(&hash)
    .bind(serde_json::to_string(&lines).unwrap_or_else(|_| "{}".into()))
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(lines)
}
