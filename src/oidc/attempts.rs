//! What a sign-in carries while it's under way, kept in memory only, and the scope it's kept under.

use super::*;

/// What a sign-in at the provider is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// The web UI signs in to its cookie. `binding` is the hash of a cookie set on the browser
    /// that started it, so nobody can finish it in someone else's browser.
    Web { binding: Vec<u8> },
    /// An app: it's sent back to `return_to` with a one-time code, which it redeems with the
    /// secret it was given at the start (hashed here).
    App { secret: Vec<u8>, return_to: String },
    /// A signed-in user links their identity at the provider to their account.
    Link { user_id: i64, binding: Vec<u8> },
}

/// A sign-in under way at the provider.
#[derive(Debug, Clone)]
pub struct Attempt {
    /// What the sign-in is for, and so where the browser goes when it ends.
    pub purpose: Purpose,
    /// The name the session it ends in is given.
    pub device_name: String,
    pub(super) address: String,
    pub(super) nonce: String,
    pub(super) verifier: String,
    pub(super) expires: Instant,
    /// The settings it began under ([`scope`]).
    pub(super) scope: Vec<u8>,
}

/// Who the provider says signed in: its issuer and its id for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The provider, as [`super::Client::issuer`] names it.
    pub issuer: String,
    /// The provider's id for the person: the ID token's `sub`.
    pub subject: String,
}

/// Sign-ins waiting for confirmation, by the hash of the browser's cookie, with the hash of
/// the page's form token and the settings they began under.
pub(super) type Confirms = HashMap<Vec<u8>, (Vec<u8>, Confirm, Vec<u8>)>;

/// How long the person in the browser has to confirm an app's sign-in.
pub(super) const CONFIRM_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// An app's sign-in that's done at the provider, waiting for the person in the browser to say
/// it's theirs before the app is sent its code.
#[derive(Debug, Clone)]
pub struct Confirm {
    /// The hash of the secret the app was given at the start.
    pub secret: Vec<u8>,
    /// The Melete user the provider's identity is linked to.
    pub user_id: i64,
    /// Their username, shown on the confirmation page.
    pub username: String,
    /// The name the app's session is given.
    pub device_name: String,
    /// Where the app is sent back to; checked when the sign-in began.
    pub return_to: String,
    /// When it runs out; [`super::Client::to_confirm`] sets it, whatever it's given.
    pub expires: Instant,
}

/// An app's sign-in that's done, waiting to be redeemed.
pub(super) struct Finished {
    pub(super) secret: Vec<u8>,
    pub(super) user_id: i64,
    pub(super) device_name: String,
    pub(super) expires: Instant,
    pub(super) scope: Vec<u8>,
}

/// Sign-ins under way, held apart from the [`Client`] so they outlast the services being set
/// up afresh: one is made at startup and handed to every client built after. Each entry
/// is kept with the settings it began under, and a client set up differently never sees it,
/// so changing the provider drops every sign-in under way rather than finishing one against
/// settings it didn't start with.
#[derive(Default)]
pub struct Pending {
    /// Keyed by the state sent to the provider.
    pub(super) attempts: Mutex<HashMap<String, Attempt>>,
    /// Keyed by the hash of the one-time code an app was sent back with.
    pub(super) finished: Mutex<HashMap<Vec<u8>, Finished>>,
    /// Keyed by the hash of the browser's cookie; with the hash of the page's form token.
    pub(super) confirms: Mutex<Confirms>,
}

/// What a client's sign-ins are kept under: a hash of every setting but the button's name, so
/// a change to any of them is another scope. Kept in memory only.
pub(super) fn scope(settings: &Oidc) -> Vec<u8> {
    let mut hash = Sha256::new();
    let parts = [
        settings.issuer.as_str(),
        settings.client_id.as_str(),
        settings.client_secret.expose(),
        settings.redirect_url.as_str(),
    ];
    for part in parts
        .into_iter()
        .chain(settings.app_returns.iter().map(String::as_str))
    {
        // Each part's length first, so no two lists of parts hash alike.
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize().to_vec()
}
