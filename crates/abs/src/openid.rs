//! ABS's mobile OpenID flow. The native client owns the session cookies and
//! PKCE verifier; the browser visits only the provider. The serializable
//! transaction belongs in credential storage so a URL can launch the app.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{Client, Login, LoginReply, Result, error};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenIdError {
    #[error("the server does not allow this app's redirect URI")]
    InvalidRedirect,
    #[error("the sign-in redirect does not match this attempt")]
    StateMismatch,
    #[error("browser sign-in was cancelled")]
    Cancelled,
    #[error("browser sign-in timed out; try again")]
    TimedOut,
    #[error("OpenID server error ({status})")]
    Server { status: u16 },
    #[error("couldn't generate secure sign-in credentials")]
    Random,
}

/// A single pending sign-in. Every field can identify or authorize the
/// transaction, so Debug never prints them. Serialize only to secure storage.
#[derive(Clone, Serialize, Deserialize)]
pub struct OpenId {
    verifier: String,
    state: String,
    redirect: String,
    cookies: String,
    browser: String,
}

impl std::fmt::Debug for OpenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpenId([redacted])")
    }
}

fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OpenIdError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

impl OpenId {
    /// The provider's URL, obtained without following ABS's redirect. Secret:
    /// pass to the system browser, never to logs or Debug.
    pub fn browser_url(&self) -> &str {
        &self.browser
    }

    pub fn used_fallback(&self) -> bool {
        self.redirect == "audiobookshelf://oauth"
    }

    /// Checks the complete redirect target and state before inspecting errors
    /// or using the code. An unrelated URL cannot cancel a pending sign-in.
    pub fn validate_callback(&self, callback: &str) -> Result<()> {
        self.code(callback).map(drop)
    }

    fn code(&self, callback: &str) -> Result<String> {
        let url = Url::parse(callback).map_err(|_| OpenIdError::StateMismatch)?;
        let mut target = url.clone();
        target.set_query(None);
        target.set_fragment(None);
        let params: Vec<_> = url.query_pairs().collect();
        let values = |name: &str| params.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_ref()).collect::<Vec<_>>();
        if target.as_str() != self.redirect || values("state") != [self.state.as_str()] || url.fragment().is_some() {
            return Err(OpenIdError::StateMismatch.into());
        }
        if values("error") == ["access_denied"] {
            return Err(OpenIdError::Cancelled.into());
        }
        if !values("error").is_empty() {
            return Err(OpenIdError::Server { status: 400 }.into());
        }
        match values("code").as_slice() {
            [code] if !code.is_empty() && *code != "undefined" => Ok((*code).to_owned()),
            _ => Err(OpenIdError::Cancelled.into()),
        }
    }
}

impl Client {
    /// Tries Rede's scheme first, falling back only on ABS's exact whitelist
    /// rejection (400, plain `Invalid redirect_uri`). No browser is opened yet.
    pub async fn begin_openid(&self) -> Result<OpenId> {
        let mut pending = OpenId { verifier: random()?, state: random()?, redirect: "rede://oauth".into(), cookies: String::new(), browser: String::new() };
        match self.authorize_openid(&mut pending).await {
            Err(crate::Error::OpenId(OpenIdError::InvalidRedirect)) => {
                pending.redirect = "audiobookshelf://oauth".into();
                self.authorize_openid(&mut pending).await?;
            }
            other => other?,
        }
        Ok(pending)
    }

    async fn authorize_openid(&self, pending: &mut OpenId) -> Result<()> {
        let mut url = self.url("auth/openid", &[]);
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &pending.redirect)
            .append_pair("code_challenge", &challenge(&pending.verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pending.state);
        // Redirects must stay disabled: following would lose the ABS session
        // cookies and could send a request to the provider before the browser.
        let http = self.openid_http()?;
        let response = http.get(url).timeout(self.inner.call_timeout).send().await?;
        let status = response.status();
        if status == StatusCode::BAD_REQUEST {
            let bytes = response.bytes().await?;
            if bytes == "Invalid redirect_uri" {
                return Err(OpenIdError::InvalidRedirect.into());
            }
            return Err(OpenIdError::Server { status: 400 }.into());
        }
        if status != StatusCode::FOUND {
            return Err(OpenIdError::Server { status: status.as_u16() }.into());
        }
        let browser = response.headers().get(header::LOCATION).and_then(|h| h.to_str().ok()).ok_or(OpenIdError::Server { status: 502 })?;
        let parsed = Url::parse(browser).map_err(|_| OpenIdError::Server { status: 502 })?;
        if !matches!(parsed.scheme(), "https" | "http") || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(OpenIdError::Server { status: 502 }.into());
        }
        pending.browser = browser.to_owned();
        pending.cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .filter_map(|h| h.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        if pending.cookies.is_empty() {
            return Err(OpenIdError::Server { status: 502 }.into());
        }
        Ok(())
    }

    fn openid_http(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(crate::CONNECT_TIMEOUT)
            .read_timeout(crate::READ_TIMEOUT)
            .user_agent(format!("{}/{}", self.inner.device.client_name, self.inner.device.client_version))
            .build()?)
    }

    /// Exchanges the provider code with ABS using the native client's original
    /// session and verifier. Success uses the password login's token hook.
    pub async fn complete_openid(&self, pending: OpenId, callback: &str) -> Result<Login> {
        let code = pending.code(callback)?;
        let mut url = self.url("auth/openid/callback", &[]);
        url.query_pairs_mut().append_pair("code", &code).append_pair("state", &pending.state).append_pair("code_verifier", &pending.verifier);
        let response = self.openid_http()?.get(url).header(header::COOKIE, &pending.cookies).timeout(self.inner.call_timeout).send().await?;
        let status = response.status();
        if !status.is_success() {
            // ABS sometimes redirects mobile failures to /login (its mobile
            // marker is a string but one branch compares it to boolean true).
            // Never follow that redirect or quote a provider's error body.
            return Err(OpenIdError::Server { status: status.as_u16() }.into());
        }
        let bytes = response.bytes().await?;
        let reply: LoginReply = error::decode_secret(&bytes).map_err(|_| OpenIdError::Server { status: 502 })?;
        let tokens = reply.tokens()?;
        self.inner.auth.replace(Some(tokens.clone())).await;
        Ok(Login {
            tokens,
            user: reply.user.user,
            default_library_id: reply.user_default_library_id,
            server_version: reply.server_settings.and_then(|s| s.version),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_matches_rfc_7636() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
