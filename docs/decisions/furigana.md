Decision: Lyrics' readings over kanji (furigana) come with their pronunciation (`pronounce`), on
the same setting, from the same built-in dictionary, as pieces of each line: a run of kanji with
its reading in hiragana, everything else as written. The kana written in a word (okurigana) are
matched in its reading and left out of the reading over the kanji; a word whose reading can't be
matched up so gets the whole reading over the whole word.

Why: The pronunciation already reads every Japanese word in context, so readings need no new
dependency, setting or outside service, and nothing leaves the server; pieces that join up into
the line let any app draw them with ruby without working out the alignment itself.

Date: 2026-11-07
