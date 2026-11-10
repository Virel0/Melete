//! The hero's picks: which titles it steps through, and in what order.

use super::*;

/// The hero's picks: what's for the viewer (their AI picks, else highly rated titles they haven't
/// started) and the newest of everything, turn about, [`HERO_FIRST`] of them; then what they're
/// partway through; then more of the first two when there's room. Each title once, carrying its
/// progress when it's partway through.
pub(super) fn picks(
    continuing: &[Entry],
    featured: &[Entry],
    highly_rated: &[Entry],
    newest: [&[(TitleRef, i64)]; 3],
) -> Vec<Entry> {
    let for_you = if featured.is_empty() {
        highly_rated
    } else {
        featured
    };
    let mut new: Vec<&(TitleRef, i64)> = newest.into_iter().flatten().collect();
    new.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let new: Vec<Entry> = new.into_iter().map(|(t, _)| Entry::plain(*t)).collect();
    let mut turn_about: Vec<&Entry> = Vec::new();
    for at in 0..for_you.len().max(new.len()) {
        turn_about.extend(for_you.get(at));
        turn_about.extend(new.get(at));
    }
    // A title they're partway through keeps its progress wherever it comes.
    let as_watched = |e: &Entry| {
        continuing
            .iter()
            .find(|c| c.title == e.title)
            .unwrap_or(e)
            .clone()
    };
    let mut seen = HashSet::new();
    let mut hero: Vec<Entry> = Vec::new();
    let mut add = |entries: &mut dyn Iterator<Item = &Entry>, up_to: usize| {
        for e in entries {
            if hero.len() >= up_to {
                break;
            }
            if seen.insert(e.title) {
                hero.push(as_watched(e));
            }
        }
    };
    add(&mut turn_about.iter().copied(), HERO_FIRST);
    add(&mut continuing.iter(), HERO);
    add(&mut turn_about.iter().copied(), HERO);
    hero
}
