//! Watch party guests: the address they reach Melete at, which invite links begin with.

use super::*;

pub(super) const PARTY: Service = Service {
    key: "party",
    name: "Watch party guests",
    group: Group::WatchParties,
    purpose: "The address guests reach Melete at",
    about: "Where someone with no account reaches Melete when a watch party invites them: usually a public address that opens only watch parties to guests (a way in with `party` alone in MELETE_WAYS_IN, through a VPS). Invite links begin with it. Nothing is sent anywhere; without it, a link begins with the address of the page that made it, which a guest outside the house may not reach.",
    fields: &[field(
        "MELETE_PARTY_URL",
        "Address for guests",
        "An address alone, e.g. https://watch.example.com.",
        Input::Text,
    )],
};
