//! Smart playlists, worked out on the server: a playlist's rules (its `smartRules`, in the
//! format Aoide's apps write) run against the library and the listener's own likes, flags and
//! listens (`music_plays`: every device's and the web's), so the web and the apps can ask
//! Melete for a smart playlist's songs and get one answer. The format and its meaning are
//! ported from the apps (iOS `CurationKit` `SmartPlaylist.swift`, Linux
//! `aoide-store/src/smart`), with their tests as the fixtures in
//! `tests/fixtures/aoide/smart-playlists.json`.
//!
//! A rule set is `{"match": "all"|"any", "rules": [{"field", "op", "value"}…],
//! "sort": {"field", "dir"}?, "limit": n?}`. Every value is a bound parameter, and fields and
//! operators reach SQL only through matches over closed enums.

use std::fmt;

use serde_json::{Map, Value as Json};

mod interval;
mod query;
mod rule;

pub use interval::Interval;
pub use query::{EvaluateError, Listener, evaluate};
pub use rule::{Field, Operator, Rule};

use rule::{malformed, required_str};

/// Why a rule or rule set was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartError {
    /// Not a field's name or alias.
    UnknownField(String),
    /// Not an operator's name or symbol.
    UnknownOperator(String),
    /// The operator doesn't compare that kind of field (`contains` on a year).
    OperatorNotApplicable { field: Field, op: Operator },
    /// The value is the wrong type for the field and operator.
    ValueTypeMismatch { field: Field, op: Operator },
    /// A date's value that isn't a span.
    MalformedInterval(String),
    /// A value that isn't text, a whole number or true/false.
    MalformedValue,
    /// `any` of no rules, which could never match anything.
    NoRules,
    /// A limit below one.
    NonPositiveLimit(i64),
    /// A sort on genre, of which a track has several.
    FieldNotSortable(Field),
    /// Not shaped like a rule set.
    MalformedJson(String),
}

impl fmt::Display for SmartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField(name) => write!(f, "no such rule field: {name}"),
            Self::UnknownOperator(name) => write!(f, "no such rule operator: {name}"),
            Self::OperatorNotApplicable { field, op } => {
                write!(
                    f,
                    "{} cannot apply to {}",
                    op.wire_name(),
                    field.wire_name()
                )
            }
            Self::ValueTypeMismatch { field, op } => write!(
                f,
                "the value does not suit {} {}",
                field.wire_name(),
                op.wire_name()
            ),
            Self::MalformedInterval(written) => write!(
                f,
                "{written} is not a relative interval; expected something like -30d, -6m or -1y"
            ),
            Self::MalformedValue => {
                write!(
                    f,
                    "a rule value must be a string, a whole number or a boolean"
                )
            }
            Self::NoRules => write!(f, "matching any of no rules can never match anything"),
            Self::NonPositiveLimit(limit) => {
                write!(f, "a limit of {limit} can never return anything")
            }
            Self::FieldNotSortable(field) => {
                write!(f, "{} has no single value to sort by", field.wire_name())
            }
            Self::MalformedJson(detail) => write!(f, "not a rule set: {detail}"),
        }
    }
}

impl std::error::Error for SmartError {}

/// How a rule set's rules combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Match {
    /// Every rule; all of none is every track.
    All,
    /// At least one rule.
    Any,
}

/// Which way a sort runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Smallest, earliest or A first.
    Asc,
    /// Largest, latest or Z first.
    Desc,
}

/// What a rule set is ordered by; ties go by track id, so a limit always takes the same songs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sort {
    /// Any field but genre.
    pub field: Field,
    /// Which way.
    pub dir: Direction,
}

/// A rule set, as a playlist's `smartRules` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rules {
    /// How the rules combine.
    pub match_mode: Match,
    /// The rules, in the order written.
    pub rules: Vec<Rule>,
    /// The order; by title when none.
    pub sort: Option<Sort>,
    /// At most this many songs; always one or more.
    pub limit: Option<i64>,
}

impl Rules {
    /// A rule set; `any` of no rules, which could only be empty, is refused.
    ///
    /// # Errors
    ///
    /// [`SmartError::NoRules`], or [`SmartError::NonPositiveLimit`] for a limit below one.
    pub fn new(
        match_mode: Match,
        rules: Vec<Rule>,
        sort: Option<Sort>,
        limit: Option<i64>,
    ) -> Result<Self, SmartError> {
        if match_mode == Match::Any && rules.is_empty() {
            return Err(SmartError::NoRules);
        }
        if let Some(limit) = limit
            && limit <= 0
        {
            return Err(SmartError::NonPositiveLimit(limit));
        }
        Ok(Self {
            match_mode,
            rules,
            sort,
            limit,
        })
    }

    /// A rule set as a value: the stored JSON text parsed, or the object itself. Operators may
    /// also be written `=`, `!=`, `>`, `<`, `>=` and `<=`.
    ///
    /// # Errors
    ///
    /// [`SmartError::MalformedJson`] when it isn't a rule set, else what a rule gives.
    pub fn from_json(value: &Json) -> Result<Self, SmartError> {
        if let Json::String(text) = value {
            let parsed: Json =
                serde_json::from_str(text).map_err(|err| malformed(&err.to_string()))?;
            return Self::from_json(&parsed);
        }
        let object = value
            .as_object()
            .ok_or_else(|| malformed("a rule set must be an object"))?;
        let match_mode = match required_str(object, "match")? {
            "all" => Match::All,
            "any" => Match::Any,
            other => return Err(malformed(&format!("'{other}' is not a match mode"))),
        };
        let rules = object
            .get("rules")
            .and_then(Json::as_array)
            .ok_or_else(|| malformed("a rule set needs a rules array"))?
            .iter()
            .map(Rule::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let sort = match object.get("sort") {
            None | Some(Json::Null) => None,
            Some(sort) => {
                let sort = sort
                    .as_object()
                    .ok_or_else(|| malformed("sort must be an object"))?;
                let field = Field::parse(required_str(sort, "field")?)?;
                let dir = match required_str(sort, "dir")? {
                    "asc" => Direction::Asc,
                    "desc" => Direction::Desc,
                    other => return Err(malformed(&format!("'{other}' is not a sort direction"))),
                };
                Some(Sort { field, dir })
            }
        };
        let limit = match object.get("limit") {
            None | Some(Json::Null) => None,
            Some(limit) => Some(
                limit
                    .as_i64()
                    .ok_or_else(|| malformed("limit must be a whole number"))?,
            ),
        };
        Self::new(match_mode, rules, sort, limit)
    }

    /// The rule set as the apps write it: snake_case fields, keys sorted.
    pub fn to_json(&self) -> Json {
        let mut map = Map::new();
        let mode = match self.match_mode {
            Match::All => "all",
            Match::Any => "any",
        };
        map.insert("match".into(), mode.into());
        map.insert(
            "rules".into(),
            Json::Array(self.rules.iter().map(Rule::to_json).collect()),
        );
        if let Some(sort) = self.sort {
            let dir = match sort.dir {
                Direction::Asc => "asc",
                Direction::Desc => "desc",
            };
            let mut by = Map::new();
            by.insert("dir".into(), dir.into());
            by.insert("field".into(), sort.field.wire_name().into());
            map.insert("sort".into(), Json::Object(by));
        }
        if let Some(limit) = self.limit {
            map.insert("limit".into(), limit.into());
        }
        Json::Object(map)
    }
}

#[cfg(test)]
mod tests;
