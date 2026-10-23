use md5::{Digest, Md5};
use rand::distr::{Alphanumeric, SampleString};

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("username", &self.username).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token: String,
    pub salt: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self { username: username.into(), password: password.into() }
    }

    /// Token for a fresh random salt; the spec asks for a new salt on every request.
    pub fn token(&self) -> Token {
        self.token_with_salt(&Alphanumeric.sample_string(&mut rand::rng(), 12))
    }

    pub fn token_with_salt(&self, salt: &str) -> Token {
        let mut h = Md5::new();
        h.update(self.password.as_bytes());
        h.update(salt.as_bytes());
        Token { token: hex::encode(h.finalize()), salt: salt.to_owned() }
    }
}
