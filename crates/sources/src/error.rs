//! What can go wrong talking to a source, sorted by what the app does about
//! it.
//!
//! [`Network`](Error::Network) means check on the server and try again later,
//! [`SignedOut`](Error::SignedOut) means the account needs the user,
//! [`Server`](Error::Server) and [`Decode`](Error::Decode) are shown as they
//! are, and [`NotSupported`](Error::NotSupported) is a request this source has
//! no answer for. The variants are a source's own vocabulary, so the app can
//! stay ignorant of which server it's talking to.
//!
//! No error carries a URL, a token or a password: a source that would have to
//! quote one reports it without.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// The server couldn't be reached, or stopped answering: no connection,
    /// a timeout, a connection dropped mid-reply.
    #[error("can't reach the server: {0}")]
    Network(String),
    /// The server answered with an error status. `message` is what it said in
    /// the body, if anything.
    #[error("{}", match message {
        Some(m) => format!("server error {status}: {m}"),
        None => format!("server error {status}"),
    })]
    Server { status: u16, message: Option<String> },
    /// There's no usable session: never signed in, or the refresh token the
    /// server would need expired or was revoked. Only signing in again fixes
    /// it.
    #[error("signed out; sign in again")]
    SignedOut,
    /// This source has no answer for the request, and no amount of retrying
    /// will change that (an item a build doesn't handle yet, an operation the
    /// server doesn't offer).
    #[error("{0}")]
    NotSupported(String),
    /// A reply that should have been JSON of a known shape and wasn't.
    #[error("malformed response: {0}")]
    Decode(String),
}

impl Error {
    /// A failure to reach the server rather than an answer from it.
    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_))
    }
    /// The account is signed out and needs the user to sign in again.
    pub fn is_signed_out(&self) -> bool {
        matches!(self, Error::SignedOut)
    }

    /// The HTTP status the server answered with, for an error that came with
    /// one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Server { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<abs::Error> for Error {
    fn from(e: abs::Error) -> Self {
        match e {
            abs::Error::Network(e) => Error::Network(e.to_string()),
            abs::Error::Server { status, message } => Error::Server { status, message },
            // The client is already signed in, so a refused token is the only
            // auth failure its calls can meet; wrong credentials and the rate
            // limiter belong to signing in, which the app does itself.
            abs::Error::Auth(_) => Error::SignedOut,
            abs::Error::Decode(said) => Error::Decode(said),
            // A reply that was a web page or something else entirely: the app
            // shows it, so the words are kept.
            abs::Error::NotApi(said) => Error::Decode(match said {
                Some(said) => format!("the server answered with a page instead of its API: {said}"),
                None => "the server answered with something other than its API".to_owned(),
            }),
            // The address was checked when the account was added, so this is
            // an account the user has to fix rather than a reply; it reads
            // like an unreachable server, which is what it is to a sync.
            abs::Error::Url(e) => Error::Network(format!("invalid server address: {e}")),
        }
    }
}
