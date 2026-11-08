//! Why background work failed, as it reaches the UI. The message is what the
//! user reads; the kind says what to do about it. Only a network failure is a
//! reason to try again quietly in the background: the server is probably gone.
//!
//! [`sources::Error`] also says when an account needs the user
//! ([`SignedOut`](sources::Error::SignedOut)) and when the server is rate
//! limiting ([`Busy`](sources::Error::Busy)). Neither is a [`Failure`]: the
//! sync handles them by signing out and by waiting, so they never reach here
//! as an error to show.

use std::fmt;

/// What the app says when the saved login no longer works.
pub const SIGNED_OUT: &str = "Signed out. Sign in again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No answer: the network is down, the server is off or slow, or a proxy
    /// in front of it answered with an error page.
    Network(String),
    /// The server answered, and refused or said something Rede can't read.
    Server(String),
    /// On this computer: the shelf database, a file.
    Local(String),
}

impl Failure {
    /// What the user reads.
    pub fn message(&self) -> &str {
        match self {
            Failure::Network(m) | Failure::Server(m) | Failure::Local(m) => m,
        }
    }

    pub fn into_message(self) -> String {
        match self {
            Failure::Network(m) | Failure::Server(m) | Failure::Local(m) => m,
        }
    }

    /// Worth trying again in the background, quietly.
    pub fn is_network(&self) -> bool {
        matches!(self, Failure::Network(_))
    }
}

impl From<sources::Error> for Failure {
    fn from(e: sources::Error) -> Self {
        match e {
            sources::Error::Network(m) => Failure::Network(m),
            sources::Error::Server { status, message } => Failure::Server(match message {
                Some(m) => format!("The server answered with an error ({status}): {m}"),
                None => format!("The server answered with an error ({status})."),
            }),
            // Handled by the sync before it gets here (a quiet wait, then a
            // retry), but if one is shown it still reads plainly.
            sources::Error::Busy { .. } => Failure::Server("The server is busy. Try again in a moment.".into()),
            sources::Error::SignedOut => Failure::Server(SIGNED_OUT.into()),
            sources::Error::NotSupported(what) => Failure::Server(what),
            sources::Error::Decode(said) => Failure::Server(format!("The server's reply couldn't be read: {said}")),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_request_that_got_no_answer_is_the_networks() {
        let refused = Failure::from(sources::Error::Server { status: 500, message: Some("no room".into()) });
        assert!(!refused.is_network());
        assert_eq!(refused.message(), "The server answered with an error (500): no room");
        assert!(Failure::from(sources::Error::Network("timed out".into())).is_network());
        assert!(!Failure::Local("disk full".into()).is_network());
    }

    #[test]
    fn a_signed_out_or_busy_server_never_reads_as_the_network() {
        let out = Failure::from(sources::Error::SignedOut);
        assert!(!out.is_network());
        assert_eq!(out.message(), SIGNED_OUT);
        let busy = Failure::from(sources::Error::Busy { retry_after: None });
        assert!(!busy.is_network());
        assert!(busy.message().contains("busy"));
    }
}
