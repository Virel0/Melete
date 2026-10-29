//! Folded keys for searching and sorting titles, stored beside them. A search folds its words
//! the same way, so "amelie" finds "Amélie", "spider man" finds "Spider-Man" and "pokemon"
//! finds "Pokémon"; a sort key leaves out a leading article, so "The Matrix" sorts under M.
//! SQLite can't fold accents itself, so the keys are worked out here and stored: a changed
//! title clears its keys (a trigger does it), and [`fill`] works out every cleared or new one.

use sqlx::SqlitePool;
use unicode_normalization::UnicodeNormalization;

/// Leading words a sort key leaves out, when more words follow: English's always, and those
/// of the language a title was fetched in (its ISO 639-1 code).
const ARTICLES: &[&str] = &["the", "a", "an"];
const LANGUAGE_ARTICLES: &[(&str, &[&str])] = &[
    ("fr", &["le", "la", "les", "un", "une"]),
    ("de", &["der", "die", "das", "ein", "eine"]),
    ("es", &["el", "la", "los", "las", "un", "una"]),
    (
        "it",
        &["il", "lo", "la", "i", "gli", "le", "un", "uno", "una"],
    ),
    ("pt", &["o", "a", "os", "as", "um", "uma"]),
    ("nl", &["de", "het", "een"]),
];

/// Languages whose article elides before a vowel ("L'Odyssée"), left out with its apostrophe.
const ELIDING: &[&str] = &["fr", "it"];

/// Joins the folded parts of a search key, so a search never matches across two of them.
const SEPARATOR: char = '|';

/// Adds one character as keys see it: lower case, a few letters spelled out ("æ" is "ae"),
/// "&" as "and", and nothing for anything else that isn't a letter or digit. `false` when
/// nothing was added.
fn fold_char(c: char, out: &mut String) -> bool {
    let start = out.len();
    for lower in c.to_lowercase() {
        match lower {
            'ø' => out.push('o'),
            'ł' => out.push('l'),
            'đ' | 'ð' => out.push('d'),
            'ı' => out.push('i'),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'ß' => out.push_str("ss"),
            'þ' => out.push_str("th"),
            '&' => out.push_str("and"),
            other if other.is_alphanumeric() => out.push(other),
            _ => {}
        }
    }
    out.len() > start
}

/// The text with its accents taken off: letters decomposed, the general combining marks
/// (the accents of Latin, Greek and Cyrillic letters) dropped, and the rest composed again,
/// so other scripts' marks (kana's voicing marks, say) still tell letters apart.
fn without_accents(text: &str) -> impl Iterator<Item = char> + '_ {
    text.nfd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .nfc()
}

/// Letters and digits only, lower case, accents gone: "Spider-Man" and "spider man" are both
/// "spiderman".
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in without_accents(text) {
        fold_char(c, &mut out);
    }
    out
}

/// The search key of a row: each part folded, empty and repeated parts left out, joined so a
/// search can't match across two parts.
pub fn search_key(parts: &[Option<&str>]) -> String {
    let mut folded: Vec<String> = Vec::new();
    for part in parts.iter().flatten() {
        let part = fold(part);
        if !part.is_empty() && !folded.contains(&part) {
            folded.push(part);
        }
    }
    folded.join(&SEPARATOR.to_string())
}

/// The sort key of a title: its words folded and spaced, a leading "the", "a" or "an" left
/// out when more words follow. "The Matrix" is "matrix", "Charlotte's Web" "charlottes web".
pub fn sort_key(title: &str) -> String {
    sort_key_in(title, None)
}

/// Like [`sort_key`], for a title in `language` (a TMDB language such as `fr-FR`), whose own
/// articles are left out too: "Le Mépris" is "mepris", "L'Odyssée" "odyssee".
pub fn sort_key_in(title: &str, language: Option<&str>) -> String {
    let code = language.map(|l| l.split('-').next().unwrap_or(l));
    let own: &[&str] = LANGUAGE_ARTICLES
        .iter()
        .find(|(c, _)| Some(*c) == code)
        .map_or(&[], |(_, articles)| articles);
    let mut title = title;
    if code.is_some_and(|c| ELIDING.contains(&c))
        && let Some(rest) = title
            .strip_prefix(['L', 'l'])
            .and_then(|r| r.strip_prefix(['\'', '’']))
        && !rest.trim().is_empty()
    {
        title = rest;
    }
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    for c in without_accents(title) {
        // Apostrophes join a word's parts rather than splitting it.
        if matches!(c, '\'' | '’' | '`') {
            continue;
        }
        if !fold_char(c, &mut word) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.len() > 1
        && (ARTICLES.contains(&words[0].as_str()) || own.contains(&words[0].as_str()))
    {
        words.remove(0);
    }
    words.join(" ")
}

/// The `LIKE` pattern a search's words make against search keys, or `None` when folding
/// leaves nothing (only punctuation), which then matches no key.
pub fn pattern(query: Option<&str>) -> Option<String> {
    let folded = fold(query?);
    // Folded text has letters and digits only, so it holds none of LIKE's wildcards.
    (!folded.is_empty()).then(|| format!("%{folded}%"))
}

/// A table with keys: its id column, the columns its search key is made of (the first also
/// gives its sort key), whether the second gives a sort key of its own, and the column saying
/// what language its titles are in.
struct Keyed {
    table: &'static str,
    id: &'static str,
    first: &'static str,
    second: Option<&'static str>,
    second_sort: Option<&'static str>,
    language: Option<&'static str>,
}

const KEYED: &[Keyed] = &[
    Keyed {
        table: "movies",
        id: "id",
        first: "parsed_title",
        second: None,
        second_sort: None,
        language: None,
    },
    Keyed {
        table: "series",
        id: "id",
        first: "parsed_title",
        second: None,
        second_sort: None,
        language: None,
    },
    Keyed {
        table: "tmdb_movies",
        id: "tmdb_id",
        first: "title",
        second: Some("original_title"),
        second_sort: None,
        language: Some("language"),
    },
    Keyed {
        table: "tmdb_series",
        id: "tmdb_id",
        first: "name",
        second: Some("original_name"),
        second_sort: None,
        language: Some("language"),
    },
    Keyed {
        table: "music_artists",
        id: "id",
        first: "name",
        second: None,
        second_sort: None,
        language: None,
    },
    Keyed {
        table: "music_albums",
        id: "id",
        first: "title",
        second: Some("artist_name"),
        second_sort: Some("artist_sort_key"),
        language: None,
    },
    Keyed {
        table: "music_tracks",
        id: "id",
        first: "title",
        second: Some("artist"),
        second_sort: None,
        language: None,
    },
];

/// Rows worked out per transaction, so a large first fill doesn't hold the database long.
const BATCH: i64 = 500;

/// Works out every key not yet worked out (new rows, changed titles), in every keyed table.
/// Returns how many rows got keys. Cheap when there's nothing to do: the rows without keys
/// have an index of their own.
pub async fn fill(pool: &SqlitePool) -> sqlx::Result<u64> {
    let mut filled = 0;
    for keyed in KEYED {
        filled += fill_table(pool, keyed).await?;
    }
    Ok(filled)
}

async fn fill_table(pool: &SqlitePool, keyed: &Keyed) -> sqlx::Result<u64> {
    let Keyed {
        table,
        id,
        first,
        second,
        second_sort,
        language,
    } = keyed;
    let second = second.unwrap_or("NULL");
    let language = language.unwrap_or("NULL");
    let select = format!(
        "SELECT {id}, {first}, {second}, {language} FROM {table}
         WHERE search_key IS NULL AND {id} > ? ORDER BY {id} LIMIT ?"
    );
    // Written only if the title is still the one read: one changed meanwhile has had its keys
    // cleared again, and the next fill works them out from the new title.
    let extra = second_sort.map_or(String::new(), |column| format!(", {column} = ?6"));
    let update = format!(
        "UPDATE {table} SET search_key = ?1, sort_key = ?2{extra}
         WHERE {id} = ?3 AND {first} IS ?4 AND {second} IS ?5"
    );
    let mut filled = 0;
    let mut after = i64::MIN;
    loop {
        let rows: Vec<(i64, String, Option<String>, Option<String>)> =
            sqlx::query_as(sqlx::AssertSqlSafe(select.as_str()))
                .bind(after)
                .bind(BATCH)
                .fetch_all(pool)
                .await?;
        let Some(last) = rows.last() else { break };
        after = last.0;
        let mut tx = pool.begin().await?;
        for (row, first_value, second_value, language) in &rows {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(update.as_str()))
                .bind(search_key(&[Some(first_value), second_value.as_deref()]))
                .bind(sort_key_in(first_value, language.as_deref()))
                .bind(row)
                .bind(first_value)
                .bind(second_value);
            if second_sort.is_some() {
                query = query.bind(second_value.as_deref().map(sort_key));
            }
            filled += query.execute(&mut *tx).await?.rows_affected();
        }
        tx.commit().await?;
        if (rows.len() as i64) < BATCH {
            break;
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_case_and_punctuation() {
        assert_eq!(fold("Amélie"), "amelie");
        assert_eq!(fold("Pokémon: The First Movie"), "pokemonthefirstmovie");
        assert_eq!(fold("Spider-Man"), "spiderman");
        assert_eq!(fold("spider man"), "spiderman");
        assert_eq!(fold("Björk"), "bjork");
        assert_eq!(fold("Søren & Æsa"), "sorenandaesa");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("千と千尋の神隠し"), "千と千尋の神隠し");
        assert_eq!(fold("が"), "が", "kana keep their voicing marks");
        assert_eq!(fold("?!"), "");
    }

    #[test]
    fn search_keys_keep_their_parts_apart() {
        assert_eq!(
            search_key(&[Some("Amélie"), Some("Le Fabuleux Destin d'Amélie Poulain")]),
            "amelie|lefabuleuxdestindameliepoulain"
        );
        assert_eq!(search_key(&[Some("Up"), Some("UP"), None]), "up");
    }

    #[test]
    fn sort_keys_leave_out_a_leading_article() {
        assert_eq!(sort_key("The Matrix"), "matrix");
        assert_eq!(sort_key("A Beautiful Mind"), "beautiful mind");
        assert_eq!(
            sort_key("An American Werewolf in London"),
            "american werewolf in london"
        );
        assert_eq!(sort_key("Charlotte's Web"), "charlottes web");
        assert_eq!(sort_key("Them!"), "them");
        assert_eq!(
            sort_key("The"),
            "the",
            "a title that is only an article keeps it"
        );
        let mut titles = ["Memento", "The Matrix", "Mad Max", "Amélie"];
        titles.sort_by_key(|t| sort_key(t));
        assert_eq!(titles, ["Amélie", "Mad Max", "The Matrix", "Memento"]);
    }

    #[test]
    fn titles_in_another_language_leave_out_its_articles_too() {
        assert_eq!(sort_key_in("Le Mépris", Some("fr-FR")), "mepris");
        assert_eq!(
            sort_key_in("L'Odyssée de Pi", Some("fr-FR")),
            "odyssee de pi"
        );
        assert_eq!(sort_key_in("Der Untergang", Some("de-DE")), "untergang");
        assert_eq!(
            sort_key_in("The Matrix", Some("de-DE")),
            "matrix",
            "English's always"
        );
        assert_eq!(sort_key_in("Die Hard", None), "die hard");
        assert_eq!(sort_key_in("L'", Some("fr-FR")), "l");
    }

    #[test]
    fn patterns_fold_the_query() {
        assert_eq!(pattern(Some("Spider Man")).as_deref(), Some("%spiderman%"));
        assert_eq!(pattern(Some("--")), None);
        assert_eq!(pattern(None), None);
    }

    #[tokio::test]
    async fn fill_works_out_new_and_changed_titles() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO tmdb_movies (tmdb_id, title, original_title, fetched_at)
                 VALUES (194, 'Amélie', 'Le Fabuleux Destin d''Amélie Poulain', 0);
             INSERT INTO music_artists (id, key, name) VALUES (1, 'name:bjork', 'Björk');",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(fill(&pool).await.unwrap(), 2);
        assert_eq!(fill(&pool).await.unwrap(), 0, "nothing left to do");
        let keys: (String, String) =
            sqlx::query_as("SELECT search_key, sort_key FROM tmdb_movies WHERE tmdb_id = 194")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            keys,
            (
                "amelie|lefabuleuxdestindameliepoulain".into(),
                "amelie".into()
            )
        );

        sqlx::query("UPDATE tmdb_movies SET title = 'The Amélie' WHERE tmdb_id = 194")
            .execute(&pool)
            .await
            .unwrap();
        let cleared: Option<String> =
            sqlx::query_scalar("SELECT search_key FROM tmdb_movies WHERE tmdb_id = 194")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(cleared, None, "a changed title clears its keys");
        assert_eq!(fill(&pool).await.unwrap(), 1);
        let sort: String =
            sqlx::query_scalar("SELECT sort_key FROM tmdb_movies WHERE tmdb_id = 194")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sort, "amelie");
    }
}
