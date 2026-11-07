//! Why background work failed, as it reaches the UI. The message is what the
//! user reads; the kind says what to do about it. Only a network failure
//! is a reason to check whether the server is still there (a database that
//! won't read says nothing about the server).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No answer: the network is down, the server is off or slow, or a proxy
    /// in front of it answered with an error page.
    Network(String),
    /// The server answered, and refused or said something Ond can't read.
    Server(String),
    /// On this computer: the library database, a file.
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

    /// Worth asking whether the server is still there.
    pub fn is_network(&self) -> bool {
        matches!(self, Failure::Network(_))
    }

    /// The same kind of failure, said as "`what`: message".
    pub fn context(self, what: &str) -> Self {
        let text = format!("{what}: {}", self.message());
        self.retold(text)
    }

    /// The same kind of failure, saying `text` instead.
    pub fn retold(&self, text: String) -> Self {
        match self {
            Failure::Network(_) => Failure::Network(text),
            Failure::Server(_) => Failure::Server(text),
            Failure::Local(_) => Failure::Local(text),
        }
    }
}

/// A request's failure. HTTP errors count as the network's, as they do when
/// pinging: an error status there most often comes from a proxy whose server
/// is down.
impl From<subsonic::Error> for Failure {
    fn from(e: subsonic::Error) -> Self {
        match e {
            subsonic::Error::Http(_) => Failure::Network(e.to_string()),
            // A login page where the API should be is the server's (or its proxy's) doing, not the network's.
            subsonic::Error::Url(_) | subsonic::Error::Decode(_) | subsonic::Error::NotApi(_) | subsonic::Error::Api { .. } => Failure::Server(e.to_string()),
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
        let refused = Failure::from(subsonic::Error::Api { code: 70, message: "Not found".into() });
        assert!(!refused.is_network());
        let context = refused.context("Couldn't load the album");
        assert_eq!(context.message(), "Couldn't load the album: server error 70: Not found");
        assert!(matches!(context, Failure::Server(_)), "keeps its kind");
        assert!(Failure::Network("timed out".into()).is_network());
        assert!(!Failure::Local("disk full".into()).is_network());
    }
}
