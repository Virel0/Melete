Decision: A watch party keeps its last change (migration 0201): play, pause, seek (a jump of 2 seconds or more from where it would be) or next (another file), who made it (a household member by account, a guest by id and the name they gave) and when. A change that leaves the party as it was isn't one, and pausing for a wait and playing on after it aren't either. Both views give it as `last_change {by, guest, you, kind, at}` (null before any), and the waits as `waits [{name, guest, you, since, ends_at}]`, with `ends_at` 30 seconds after `since`; `waiting` stays as it was. A guest is told "someone from the household" for a household member in both, never an account's name.

Why: A guest looking at a still picture couldn't tell whether someone paused, jumped, or the party was waiting, or for how long. Keeping who and what on the party lets every player say so in words. A guest's view keeps the rule that it shows no account's name until a decision changes that.

Date: 2026-11-28
