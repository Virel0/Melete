//! Make room's settings: the studios to count as major beside the built-in list.

use super::*;

pub(super) const MAKE_ROOM: Service = Service {
    key: "make_room",
    name: "Make room",
    about: "Suggestions for freeing space in Movies, Series and Anime: titles to delete, swap for a smaller release, or compress, with the space each frees. A major studio's titles are suggested less, as are titles watched lately or well rated; nothing anyone is watching, has on their list or asked for lately is suggested. Worked out on the server from what it knows; nothing leaves it.",
    fields: &[field(
        "MELETE_MAJOR_STUDIOS",
        "More major studios",
        "Names to count as major studios besides the built-in list (Disney, Warner Bros., Universal, Paramount, Sony and the major anime studios among them), separated by commas. A name matches a company's name containing it, in any case.",
        Input::Text,
    )],
};
