//! Quick Connect: a new device signs in without anyone typing a password on it. The
//! device asks for a code and shows it; someone signed in elsewhere looks the code up, sees
//! which device it is, where it asked from and when, and approves it as them; the device,
//! asking with the secret only it was given, then gets a session of its own, as a password
//! sign-in would give it.
//!
//! Codes live in memory only: they last five minutes and are used once, so a restart losing
//! them costs a device one more try. The code is what a person reads off a screen; the secret
//! is what the device proves itself with, so knowing a code someone shows doesn't let anyone
//! else collect the session. Neither is ever logged or shown again. Looking codes up is what a
//! guesser would do, so each user gets ten lookups an hour, found or not; approving needs a
//! lookup of that code by the same user first.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::anyhow;

/// How long a code can be approved and, once approved, collected.
pub const LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Codes waiting at once, in all, and from one address (an IPv6 /64 counting as one). The last
/// `HOME_RESERVE` places are kept for requests through a way in that opens everything (the
/// household's own), so starts from outside can't use them all up.
const MAX_PENDING: usize = 100;
const HOME_RESERVE: usize = 20;
const MAX_PENDING_PER_ADDRESS: usize = 5;
/// Lookups a user may make an hour, whether the code is found or not.
pub const LOOKUPS_PER_HOUR: usize = 10;
const HOUR: Duration = Duration::from_secs(3600);
/// Letters and digits that can't be mistaken for one another on a TV across the room.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 6;
/// What a device's secret starts with; anything else is unknown without a lookup.
pub const SECRET_PREFIX: &str = "mqc_";

struct Pending {
    code: String,
    device_name: String,
    /// Where it asked from, as shown to whoever looks it up.
    address: String,
    created: Instant,
    expires: Instant,
    /// Users who looked it up; only they can approve it.
    seen_by: Vec<i64>,
    /// The user who approved it, once someone has.
    approved_by: Option<i64>,
}

/// A code just made: what the device shows, and what it asks with.
#[derive(Debug)]
pub struct Started {
    /// What the device shows, for someone signed in to look up and approve.
    pub code: String,
    /// What the device polls with. Only its hash is kept; it's never shown again.
    pub secret: String,
}

/// Why a code wasn't started.
#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// Too many codes are waiting, in all or from this address.
    Busy,
}

/// What looking a code up shows, before anyone approves it.
#[derive(Debug, PartialEq, Eq)]
pub struct Seen {
    /// The name the device gave.
    pub device_name: String,
    /// Where it asked from.
    pub address: String,
    /// How long ago it asked.
    pub age: Duration,
    /// How long is left to approve it.
    pub expires_in: Duration,
}

/// Why looking a code up showed nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum LookupError {
    /// No code waiting is that one.
    NotFound,
    /// The user has looked up ten codes this hour.
    Limited,
}

/// What asking with a secret finds.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// Nobody has approved it yet.
    Waiting,
    /// Approved: the code is gone now, and the device gets a session for this user, named so.
    Approved { user_id: i64, device_name: String },
    /// No such secret, or its code ran out.
    Unknown,
}

/// The address an address counts as for caps: itself, or its /64 network for IPv6, since one
/// household or one attacker gets a whole /64.
pub fn address_group(address: &str) -> String {
    match address
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.to_canonical())
    {
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        Ok(ip) => ip.to_string(),
        Err(_) => address.to_owned(),
    }
}

/// The codes waiting, and each user's recent lookups. In memory only; one is made at startup.
#[derive(Default)]
pub struct QuickConnect {
    /// Keyed by the SHA-256 of the secret, so a lookup's timing says nothing about it.
    pending: Mutex<HashMap<Vec<u8>, Pending>>,
    /// When each user looked codes up, in the last hour.
    lookups: Mutex<HashMap<i64, VecDeque<Instant>>>,
}

impl QuickConnect {
    /// Starts a code for a device asking from `address`; `home` when it came in through a way in
    /// that opens everything (or none are set), which may use the places kept for home.
    pub fn start(
        &self,
        device_name: &str,
        address: &str,
        home: bool,
    ) -> anyhow::Result<Result<Started, StartError>> {
        self.start_from(device_name, address, home, Instant::now())
    }

    #[cfg(test)]
    fn start_at(
        &self,
        device_name: &str,
        address: &str,
        now: Instant,
    ) -> anyhow::Result<Result<Started, StartError>> {
        self.start_from(device_name, address, true, now)
    }

    fn start_from(
        &self,
        device_name: &str,
        address: &str,
        home: bool,
        now: Instant,
    ) -> anyhow::Result<Result<Started, StartError>> {
        let mut pending = self.pending.lock().expect("no holder panics");
        pending.retain(|_, p| p.expires > now);
        let group = address_group(address);
        let from_here = pending
            .values()
            .filter(|p| address_group(&p.address) == group)
            .count();
        let room = if home {
            MAX_PENDING
        } else {
            MAX_PENDING - HOME_RESERVE
        };
        if pending.len() >= room || from_here >= MAX_PENDING_PER_ADDRESS {
            return Ok(Err(StartError::Busy));
        }
        let code = loop {
            let code = random_code()?;
            if !pending.values().any(|p| p.code == code) {
                break code;
            }
        };
        let secret = crate::auth::random_token(SECRET_PREFIX)?;
        pending.insert(
            crate::auth::token_hash(&secret),
            Pending {
                code: code.clone(),
                device_name: device_name.to_owned(),
                address: address.to_owned(),
                created: now,
                expires: now + LIFETIME,
                seen_by: Vec::new(),
                approved_by: None,
            },
        );
        Ok(Ok(Started { code, secret }))
    }

    /// Looks a code up for `user_id`: which device it is, where it asked from and how long
    /// ago. Every lookup counts toward the user's ten an hour, found or not.
    pub fn lookup(&self, code: &str, user_id: i64) -> Result<Seen, LookupError> {
        self.lookup_at(code, user_id, Instant::now())
    }

    fn lookup_at(&self, code: &str, user_id: i64, now: Instant) -> Result<Seen, LookupError> {
        {
            let mut lookups = self.lookups.lock().expect("no holder panics");
            lookups.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < HOUR));
            let times = lookups.entry(user_id).or_default();
            while times
                .front()
                .is_some_and(|t| now.duration_since(*t) >= HOUR)
            {
                times.pop_front();
            }
            if times.len() >= LOOKUPS_PER_HOUR {
                return Err(LookupError::Limited);
            }
            times.push_back(now);
        }
        let code = normalize(code).ok_or(LookupError::NotFound)?;
        let mut pending = self.pending.lock().expect("no holder panics");
        let entry = pending
            .values_mut()
            .find(|p| p.code == code && p.expires > now && p.approved_by.is_none())
            .ok_or(LookupError::NotFound)?;
        if !entry.seen_by.contains(&user_id) {
            entry.seen_by.push(user_id);
        }
        Ok(Seen {
            device_name: entry.device_name.clone(),
            address: entry.address.clone(),
            age: now.duration_since(entry.created),
            expires_in: entry.expires.duration_since(now),
        })
    }

    /// Approves the code as `user_id`, answering the device's name; None when no code waiting
    /// is that one, or this user hasn't looked it up. A code already approved can't be
    /// approved again, by anyone.
    pub fn approve(&self, code: &str, user_id: i64) -> Option<String> {
        self.approve_at(code, user_id, Instant::now())
    }

    fn approve_at(&self, code: &str, user_id: i64, now: Instant) -> Option<String> {
        let code = normalize(code)?;
        let mut pending = self.pending.lock().expect("no holder panics");
        let entry = pending.values_mut().find(|p| {
            p.code == code
                && p.expires > now
                && p.approved_by.is_none()
                && p.seen_by.contains(&user_id)
        })?;
        entry.approved_by = Some(user_id);
        Some(entry.device_name.clone())
    }

    /// What the device holding `secret` gets now. An approved code is handed over once and
    /// removed in the same step, so two askers can't both get a session.
    pub fn poll(&self, secret: &str) -> Poll {
        self.poll_at(secret, Instant::now())
    }

    fn poll_at(&self, secret: &str, now: Instant) -> Poll {
        if !secret.starts_with(SECRET_PREFIX) {
            return Poll::Unknown;
        }
        let key = crate::auth::token_hash(secret);
        let mut pending = self.pending.lock().expect("no holder panics");
        let Entry::Occupied(entry) = pending.entry(key) else {
            return Poll::Unknown;
        };
        if entry.get().expires <= now {
            entry.remove();
            return Poll::Unknown;
        }
        let Some(user_id) = entry.get().approved_by else {
            return Poll::Waiting;
        };
        Poll::Approved {
            user_id,
            device_name: entry.remove().device_name,
        }
    }
}

fn random_code() -> anyhow::Result<String> {
    let mut bytes = [0u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no randomness available: {e}"))?;
    // 32 letters divide 256 evenly, so every code is as likely as any other.
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect())
}

/// A code as a person types it: any case, with spaces or a dash between its halves. None
/// when it can't be a code at all.
fn normalize(code: &str) -> Option<String> {
    let code: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == CODE_LENGTH && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_short_and_readable_and_secrets_long() {
        let store = QuickConnect::default();
        let started = store
            .start("Living room TV", "192.168.1.10", true)
            .unwrap()
            .unwrap();
        assert_eq!(started.code.len(), CODE_LENGTH);
        assert!(started.code.bytes().all(|b| ALPHABET.contains(&b)));
        assert!(!started.code.contains(['0', 'O', '1', 'I']));
        assert!(started.secret.starts_with(SECRET_PREFIX));
        assert_eq!(started.secret.len(), SECRET_PREFIX.len() + 64);
    }

    #[test]
    fn a_code_is_looked_up_then_approved_and_handed_over_once() {
        let store = QuickConnect::default();
        let t0 = Instant::now();
        let started = store
            .start_at("Living room TV", "192.168.1.10", t0)
            .unwrap()
            .unwrap();
        assert_eq!(store.poll(&started.secret), Poll::Waiting);
        assert_eq!(
            store.approve(&started.code, 7),
            None,
            "not before it's been looked up"
        );
        // Typed as a person might.
        let typed = format!(
            " {}-{} ",
            started.code[..3].to_lowercase(),
            &started.code[3..]
        );
        let seen = store
            .lookup_at(&typed, 7, t0 + Duration::from_secs(30))
            .unwrap();
        assert_eq!(
            seen,
            Seen {
                device_name: "Living room TV".into(),
                address: "192.168.1.10".into(),
                age: Duration::from_secs(30),
                expires_in: LIFETIME - Duration::from_secs(30),
            }
        );
        assert_eq!(
            store.approve(&started.code, 8),
            None,
            "only by someone who looked it up"
        );
        assert_eq!(store.approve(&typed, 7), Some("Living room TV".into()));
        assert_eq!(store.approve(&started.code, 7), None, "approved once only");
        assert_eq!(
            store.lookup(&started.code, 8),
            Err(LookupError::NotFound),
            "an approved code isn't shown again"
        );
        assert_eq!(
            store.poll(&started.secret),
            Poll::Approved {
                user_id: 7,
                device_name: "Living room TV".into()
            }
        );
        assert_eq!(store.poll(&started.secret), Poll::Unknown, "used once");
    }

    #[test]
    fn the_code_alone_collects_nothing() {
        let store = QuickConnect::default();
        let started = store.start("tv", "192.168.1.10", true).unwrap().unwrap();
        store.lookup(&started.code, 1).unwrap();
        store.approve(&started.code, 1).unwrap();
        for guess in [started.code.as_str(), "mqc_00", "", "mlt_x"] {
            assert_eq!(store.poll(guess), Poll::Unknown, "{guess}");
        }
    }

    #[test]
    fn lookups_are_limited_whether_they_find_a_code_or_not() {
        let store = QuickConnect::default();
        let t0 = Instant::now();
        // Approving codes of one's own doesn't buy more guesses.
        for _ in 0..LOOKUPS_PER_HOUR / 2 {
            let mine = store.start_at("tv", "192.168.1.10", t0).unwrap().unwrap();
            store.lookup_at(&mine.code, 1, t0).unwrap();
            assert!(store.approve_at(&mine.code, 1, t0).is_some());
            store.poll_at(&mine.secret, t0);
            assert_eq!(store.lookup_at("AAAAAA", 1, t0), Err(LookupError::NotFound));
        }
        let theirs = store.start_at("tv", "192.168.1.11", t0).unwrap().unwrap();
        assert_eq!(
            store.lookup_at(&theirs.code, 1, t0),
            Err(LookupError::Limited)
        );
        assert!(store.lookup_at(&theirs.code, 2, t0).is_ok(), "per user");
        assert_eq!(
            store.lookup_at(&theirs.code, 1, t0 + Duration::from_secs(3600)),
            Err(LookupError::NotFound),
            "an hour on it's looked for again (and has run out by then)"
        );
    }

    #[test]
    fn codes_run_out_after_five_minutes() {
        let store = QuickConnect::default();
        let t0 = Instant::now();
        let waiting = store.start_at("a", "192.168.1.10", t0).unwrap().unwrap();
        let approved = store.start_at("b", "192.168.1.10", t0).unwrap().unwrap();
        let late = t0 + LIFETIME;
        store.lookup_at(&approved.code, 1, t0).unwrap();
        assert_eq!(store.approve_at(&approved.code, 1, t0), Some("b".into()));
        assert_eq!(
            store.lookup_at(&waiting.code, 1, late),
            Err(LookupError::NotFound)
        );
        assert_eq!(store.poll_at(&waiting.secret, late), Poll::Unknown);
        assert_eq!(
            store.poll_at(&approved.secret, late),
            Poll::Unknown,
            "an approval not collected in time is lost"
        );
    }

    #[test]
    fn only_so_many_codes_wait_at_once() {
        let store = QuickConnect::default();
        let t0 = Instant::now();
        for _ in 0..MAX_PENDING_PER_ADDRESS {
            store.start_at("tv", "192.168.1.10", t0).unwrap().unwrap();
        }
        assert_eq!(
            store
                .start_at("tv", "192.168.1.10", t0)
                .unwrap()
                .unwrap_err(),
            StartError::Busy
        );
        assert!(
            store.start_at("tv", "192.168.1.11", t0).unwrap().is_ok(),
            "another address isn't held up"
        );
        assert!(
            store
                .start_at("tv", "192.168.1.10", t0 + LIFETIME)
                .unwrap()
                .is_ok(),
            "run-out codes make room"
        );
        for n in 0..MAX_PENDING {
            let _ = store.start_at(
                "tv",
                &format!("10.0.{}.{}", n / 200, n % 200),
                t0 + LIFETIME,
            );
        }
        assert_eq!(
            store
                .start_at("tv", "10.9.9.9", t0 + LIFETIME)
                .unwrap()
                .unwrap_err(),
            StartError::Busy
        );
    }

    #[test]
    fn an_ipv6_network_counts_as_one_address() {
        assert_eq!(address_group("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert_eq!(address_group("::ffff:192.168.1.10"), "192.168.1.10");
        assert_eq!(address_group("192.168.1.10"), "192.168.1.10");
        let store = QuickConnect::default();
        for n in 0..MAX_PENDING_PER_ADDRESS {
            store
                .start(&format!("tv {n}"), &format!("2001:db8:1:2::{n:x}"), true)
                .unwrap()
                .unwrap();
        }
        assert_eq!(
            store
                .start("tv", "2001:db8:1:2:ffff::9", true)
                .unwrap()
                .unwrap_err(),
            StartError::Busy,
            "another address in the same /64"
        );
        assert!(store.start("tv", "2001:db8:1:3::1", true).unwrap().is_ok());
    }

    #[test]
    fn places_are_kept_for_home_when_outside_fills_the_rest() {
        let store = QuickConnect::default();
        let t0 = Instant::now();
        for n in 0..MAX_PENDING - HOME_RESERVE {
            let address = format!("203.0.{}.{}", n / 200, n % 200);
            store
                .start_from("tv", &address, false, t0)
                .unwrap()
                .unwrap();
        }
        assert_eq!(
            store
                .start_from("tv", "203.0.9.9", false, t0)
                .unwrap()
                .unwrap_err(),
            StartError::Busy,
            "outside has used its share"
        );
        for n in 0..HOME_RESERVE {
            let address = format!("192.168.1.{n}");
            store.start_from("tv", &address, true, t0).unwrap().unwrap();
        }
        assert_eq!(
            store
                .start_from("tv", "192.168.2.1", true, t0)
                .unwrap()
                .unwrap_err(),
            StartError::Busy,
            "and home its own"
        );
    }

    #[test]
    fn typed_codes_are_read_forgivingly_but_never_loosely() {
        assert_eq!(normalize("abc-def"), Some("ABCDEF".into()));
        assert_eq!(normalize("AB CD EF"), Some("ABCDEF".into()));
        for bad in ["ABCDE", "ABCDEFG", "ABCDE0", "ÄBCDEF", ""] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
    }
}
