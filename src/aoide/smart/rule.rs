//! One rule: the field it talks about, the comparison, and the value, and which combinations
//! of the three are legal. Ported from the apps (iOS `SmartPlaylist.swift`, Linux
//! `aoide-store/src/smart/rule.rs`): an illegal pair is refused by name, never quietly
//! matching nothing.

use serde_json::{Map, Value as Json};

use super::SmartError;
use super::interval::Interval;

/// What a rule may talk about. Written in snake_case; camelCase is accepted on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// The track's title. Text.
    Title,
    /// Its credited artists, joined with ", " (else its album's artist). Text.
    Artist,
    /// Its album's title. Text.
    Album,
    /// Its album's artist; none compares as empty text.
    AlbumArtist,
    /// Any one of its genres. Text; not sortable.
    Genre,
    /// Its year (else its album's). A number; unknown matches only `isNot`.
    Year,
    /// Its length in milliseconds. A number.
    DurationMs,
    /// The listener's listens that count as plays. A number; zero when never played.
    PlayCount,
    /// The listener's listens that count as skips. A number.
    SkipCount,
    /// When the listener last played it, in milliseconds. A date; never played has none.
    LastPlayed,
    /// When the listener first played it. A date.
    FirstPlayed,
    /// Whether the listener likes it. A flag.
    Liked,
}

/// What kind of value a field holds, which decides its operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Text,
    Number,
    Date,
    Flag,
}

impl Field {
    /// Every field, in the apps' order.
    pub const ALL: [Field; 12] = [
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::AlbumArtist,
        Self::Genre,
        Self::Year,
        Self::DurationMs,
        Self::PlayCount,
        Self::SkipCount,
        Self::LastPlayed,
        Self::FirstPlayed,
        Self::Liked,
    ];

    /// How it's written.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::AlbumArtist => "album_artist",
            Self::Genre => "genre",
            Self::Year => "year",
            Self::DurationMs => "duration_ms",
            Self::PlayCount => "play_count",
            Self::SkipCount => "skip_count",
            Self::LastPlayed => "last_played",
            Self::FirstPlayed => "first_played",
            Self::Liked => "liked",
        }
    }

    /// A field by its written name or a camelCase alias (`albumArtist`, `playCount`…).
    ///
    /// # Errors
    ///
    /// [`SmartError::UnknownField`] for any other name.
    pub fn parse(wire: &str) -> Result<Self, SmartError> {
        let alias = match wire {
            "albumArtist" => Some(Self::AlbumArtist),
            "durationMs" => Some(Self::DurationMs),
            "playCount" => Some(Self::PlayCount),
            "skipCount" => Some(Self::SkipCount),
            "lastPlayed" => Some(Self::LastPlayed),
            "firstPlayed" => Some(Self::FirstPlayed),
            _ => None,
        };
        Self::ALL
            .into_iter()
            .find(|field| field.wire_name() == wire)
            .or(alias)
            .ok_or_else(|| SmartError::UnknownField(wire.to_owned()))
    }

    pub(super) fn kind(self) -> Kind {
        match self {
            Self::Title | Self::Artist | Self::Album | Self::AlbumArtist | Self::Genre => {
                Kind::Text
            }
            Self::Year | Self::DurationMs | Self::PlayCount | Self::SkipCount => Kind::Number,
            Self::LastPlayed | Self::FirstPlayed => Kind::Date,
            Self::Liked => Kind::Flag,
        }
    }
}

/// The comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operator {
    /// Equal to; text compares case-insensitively (ASCII only, as SQLite does).
    Is,
    /// Not equal to; a missing value counts as not equal.
    IsNot,
    /// Text containing the value.
    Contains,
    /// Text not containing the value.
    DoesNotContain,
    /// Text starting with the value.
    BeginsWith,
    /// A number greater than the value.
    GreaterThan,
    /// A number less than the value.
    LessThan,
    /// A date before an instant (relative, or epoch milliseconds); never played fails it.
    Before,
    /// A date after an instant; never played fails it.
    After,
    /// A date within a span back from now.
    InTheLast,
    /// A date outside a span back from now, or none at all.
    NotInTheLast,
}

impl Operator {
    /// Every operator, in the apps' order.
    pub const ALL: [Operator; 11] = [
        Self::Is,
        Self::IsNot,
        Self::Contains,
        Self::DoesNotContain,
        Self::BeginsWith,
        Self::GreaterThan,
        Self::LessThan,
        Self::Before,
        Self::After,
        Self::InTheLast,
        Self::NotInTheLast,
    ];

    /// How it's written.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Is => "is",
            Self::IsNot => "isNot",
            Self::Contains => "contains",
            Self::DoesNotContain => "doesNotContain",
            Self::BeginsWith => "beginsWith",
            Self::GreaterThan => "greaterThan",
            Self::LessThan => "lessThan",
            Self::Before => "before",
            Self::After => "after",
            Self::InTheLast => "inTheLast",
            Self::NotInTheLast => "notInTheLast",
        }
    }

    /// A name, or a symbol and how far it moves the bound: `>=` and `<=` are exact on the
    /// whole numbers every comparable field holds.
    fn resolve(wire: &str) -> Result<(Self, i64), SmartError> {
        if let Some(op) = Self::ALL.into_iter().find(|op| op.wire_name() == wire) {
            return Ok((op, 0));
        }
        match wire {
            "=" | "==" => Ok((Self::Is, 0)),
            "!=" | "<>" => Ok((Self::IsNot, 0)),
            ">" => Ok((Self::GreaterThan, 0)),
            "<" => Ok((Self::LessThan, 0)),
            ">=" => Ok((Self::GreaterThan, -1)),
            "<=" => Ok((Self::LessThan, 1)),
            _ => Err(SmartError::UnknownOperator(wire.to_owned())),
        }
    }
}

/// A rule's value: text, a whole number or a flag; never a fraction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// For text fields, and spans (`-30d`) for dates.
    Text(String),
    /// For number fields, and epoch milliseconds for `before` and `after`.
    Number(i64),
    /// For `liked`.
    Flag(bool),
}

impl Value {
    pub(super) fn to_json(&self) -> Json {
        match self {
            Self::Text(text) => Json::String(text.clone()),
            Self::Number(number) => Json::from(*number),
            Self::Flag(flag) => Json::Bool(*flag),
        }
    }

    fn from_json(value: &Json) -> Result<Self, SmartError> {
        match value {
            Json::Bool(flag) => Ok(Self::Flag(*flag)),
            Json::Number(number) => number
                .as_i64()
                .map(Self::Number)
                .ok_or(SmartError::MalformedValue),
            Json::String(text) => Ok(Self::Text(text.clone())),
            _ => Err(SmartError::MalformedValue),
        }
    }
}

/// One clause: a field, how it compares, and with what.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rule {
    /// What it compares.
    pub field: Field,
    /// How.
    pub op: Operator,
    /// With what; its type suits the field and operator.
    pub value: Value,
}

impl Rule {
    /// A rule, when the operator applies to the field and the value suits both.
    ///
    /// # Errors
    ///
    /// [`SmartError::OperatorNotApplicable`], [`SmartError::ValueTypeMismatch`], or for a
    /// date, [`SmartError::MalformedInterval`] when the text isn't a span.
    pub fn new(field: Field, op: Operator, value: Value) -> Result<Self, SmartError> {
        check(field, op, &value)?;
        Ok(Self { field, op, value })
    }

    pub(super) fn to_json(&self) -> Json {
        let mut map = Map::new();
        map.insert("field".into(), self.field.wire_name().into());
        map.insert("op".into(), self.op.wire_name().into());
        map.insert("value".into(), self.value.to_json());
        Json::Object(map)
    }

    pub(super) fn from_json(value: &Json) -> Result<Self, SmartError> {
        let object = value
            .as_object()
            .ok_or_else(|| malformed("a rule must be an object"))?;
        let field = Field::parse(required_str(object, "field")?)?;
        let (op, bias) = Operator::resolve(required_str(object, "op")?)?;
        let mut value = Value::from_json(
            object
                .get("value")
                .ok_or_else(|| malformed("a rule needs a value"))?,
        )?;
        if bias != 0 {
            let Value::Number(bound) = value else {
                return Err(SmartError::ValueTypeMismatch { field, op });
            };
            value = Value::Number(bound.saturating_add(bias));
        }
        Self::new(field, op, value)
    }
}

fn check(field: Field, op: Operator, value: &Value) -> Result<(), SmartError> {
    let require = |allowed: &[Operator]| {
        if allowed.contains(&op) {
            Ok(())
        } else {
            Err(SmartError::OperatorNotApplicable { field, op })
        }
    };
    let mismatch = SmartError::ValueTypeMismatch { field, op };
    match field.kind() {
        Kind::Text => {
            use Operator::*;
            require(&[Is, IsNot, Contains, DoesNotContain, BeginsWith])?;
            if !matches!(value, Value::Text(_)) {
                return Err(mismatch);
            }
        }
        Kind::Number => {
            use Operator::*;
            require(&[Is, IsNot, GreaterThan, LessThan])?;
            if !matches!(value, Value::Number(_)) {
                return Err(mismatch);
            }
        }
        Kind::Date => {
            use Operator::*;
            require(&[Before, After, InTheLast, NotInTheLast])?;
            match (op, value) {
                // `before` and `after` name an instant, relative or in epoch milliseconds;
                // the others a span, and a bare number is a span in no unit.
                (Before | After, Value::Number(_)) => {}
                (_, Value::Text(written)) => {
                    Interval::parse(written)?;
                }
                _ => return Err(mismatch),
            }
        }
        Kind::Flag => {
            require(&[Operator::Is, Operator::IsNot])?;
            if !matches!(value, Value::Flag(_)) {
                return Err(mismatch);
            }
        }
    }
    Ok(())
}

pub(super) fn required_str<'a>(
    object: &'a Map<String, Json>,
    key: &str,
) -> Result<&'a str, SmartError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| malformed(&format!("'{key}' must be a string")))
}

pub(super) fn malformed(detail: &str) -> SmartError {
    SmartError::MalformedJson(detail.to_owned())
}
