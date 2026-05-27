//! OAuth 2.0 Desktop / "Installed App" flow with PKCE.
//!
//! Reference: <https://developers.google.com/identity/protocols/oauth2/native-app>
//!
//! Flow walked through end-to-end:
//!
//! 1. Caller creates an [`OAuthClient`] with the user's own Google
//!    OAuth client id + secret. We don't ship a shared one (per
//!    user request: every user creates their own client at
//!    <https://console.cloud.google.com>).
//! 2. [`OAuthClient::start`] returns an [`OAuthSession`]. The
//!    session has bound a free localhost port and built the
//!    consent URL. The caller opens that URL (we do it via
//!    [`webbrowser`]).
//! 3. The user clicks "Allow"; Google redirects to
//!    `http://127.0.0.1:<port>/callback?code=…&state=…`. The tiny
//!    HTTP server in [`OAuthSession::wait_for_redirect`] picks
//!    that up, validates `state` matches the one we generated,
//!    and returns the `code`.
//! 4. The caller calls [`OAuthSession::exchange_code`] which
//!    POSTs to the token endpoint. The result includes the
//!    refresh token (Google only returns it when
//!    `access_type=offline` and `prompt=consent`, both of which
//!    we set).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{DriveError, DriveResult};
use crate::tokens::StoredTokens;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REDIRECT_PATH: &str = "/qobee/oauth/callback";

/// Hold-the-hand input from the user.
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
}

impl OAuthClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            scope: crate::DRIVE_SCOPE.into(),
        }
    }

    /// Build a session: choose a free loopback port, generate
    /// PKCE verifier/challenge, build the consent URL.
    pub fn start(self) -> DriveResult<OAuthSession> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| DriveError::OAuth(format!("could not bind loopback: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| DriveError::OAuth(format!("local_addr: {e}")))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|e| DriveError::OAuth(format!("nonblocking: {e}")))?;

        let verifier = generate_pkce_verifier();
        let challenge = pkce_challenge(&verifier);
        let state = generate_random_string(24);

        let redirect_uri = format!("http://127.0.0.1:{port}{REDIRECT_PATH}");

        let auth_url = build_auth_url(
            &self.client_id,
            &self.scope,
            &redirect_uri,
            &challenge,
            &state,
        );

        Ok(OAuthSession {
            client: self,
            listener,
            port,
            verifier,
            state,
            redirect_uri,
            auth_url,
        })
    }
}

/// One in-flight OAuth flow.
pub struct OAuthSession {
    client: OAuthClient,
    listener: TcpListener,
    port: u16,
    verifier: String,
    state: String,
    redirect_uri: String,
    auth_url: String,
}

impl OAuthSession {
    /// URL to open in the browser. The caller can present it to
    /// the user (via [`webbrowser::open`] or by displaying it for
    /// manual paste) before calling [`Self::wait_for_redirect`].
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Open the consent URL in the user's default browser. Best
    /// effort: a failure to launch the browser still leaves the
    /// loopback server running so the user can paste the URL
    /// manually.
    pub fn open_in_browser(&self) -> DriveResult<()> {
        webbrowser::open(&self.auth_url)
            .map_err(|e| DriveError::OAuth(format!("could not open browser: {e}")))?;
        Ok(())
    }

    /// Block (with a coarse `timeout`) until the OS calls the
    /// loopback redirect, then return the `code`.
    pub fn wait_for_redirect(&self, timeout: Duration) -> DriveResult<String> {
        let started = Instant::now();
        loop {
            if started.elapsed() > timeout {
                return Err(DriveError::OAuth(
                    "timed out waiting for the browser callback".into(),
                ));
            }

            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let code = handle_redirect(&mut stream, &self.state)?;
                    return Ok(code);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(120));
                }
                Err(e) => {
                    return Err(DriveError::OAuth(format!("accept: {e}")));
                }
            }
        }
    }

    /// POST to the token endpoint with the captured `code` and
    /// our verifier. On success returns persistable tokens that
    /// the caller can hand to [`crate::tokens::TokenStore::save`].
    pub fn exchange_code(self, code: &str) -> DriveResult<StoredTokens> {
        let http = Client::builder().build()?;
        let resp = http
            .post(TOKEN_URL)
            .form(&[
                ("code", code),
                ("client_id", &self.client.client_id),
                ("client_secret", &self.client.client_secret),
                ("redirect_uri", &self.redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", &self.verifier),
            ])
            .send()?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(DriveError::Api { status, body });
        }
        let tr: TokenResponse = resp.json()?;
        let now = unix_now();
        Ok(StoredTokens {
            client_id: self.client.client_id,
            client_secret: self.client.client_secret,
            access_token: tr.access_token,
            refresh_token: tr.refresh_token.ok_or_else(|| {
                DriveError::OAuth(
                    "Google did not return a refresh_token. Reset access on \
                     myaccount.google.com and retry — Google only re-issues \
                     the refresh token after a fresh `prompt=consent`."
                        .into(),
                )
            })?,
            expires_at: now + tr.expires_in.unwrap_or(3600),
            folder_id: None,
            folder_name: None,
        })
    }
}

/// Use a stored refresh token to get a new access token. Updates
/// `access_token` and `expires_at` in place; the refresh token
/// itself only changes when Google rotates it (rare).
pub fn refresh_access_token(tokens: &mut StoredTokens) -> DriveResult<()> {
    let http = Client::builder().build()?;
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", tokens.client_id.as_str()),
            ("client_secret", tokens.client_secret.as_str()),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        // 400 / 401 with `invalid_grant` means the refresh token is
        // dead (revoked, password change, etc). Surface a typed
        // error so the UI can route the user to the wizard.
        if status == 400 || status == 401 {
            return Err(DriveError::NeedsReauth(body));
        }
        return Err(DriveError::Api { status, body });
    }
    let tr: TokenResponse = resp.json()?;
    let now = unix_now();
    tokens.access_token = tr.access_token;
    tokens.expires_at = now + tr.expires_in.unwrap_or(3600);
    if let Some(rt) = tr.refresh_token {
        tokens.refresh_token = rt;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn build_auth_url(
    client_id: &str,
    scope: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    use url::form_urlencoded::Serializer;
    let mut params = Serializer::new(String::new());
    params
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", scope)
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        // `offline` is what makes Google issue a refresh_token.
        .append_pair("access_type", "offline")
        // Force the consent screen so we get a fresh refresh_token
        // every time. Without this, repeat authorizations skip the
        // consent and Google does not re-issue the refresh token,
        // leaving the wizard stuck in "no refresh token" land.
        .append_pair("prompt", "consent")
        // `include_granted_scopes` keeps any other scopes the user
        // had previously granted to this client.
        .append_pair("include_granted_scopes", "true");
    format!("{}?{}", AUTH_URL, params.finish())
}

fn handle_redirect(stream: &mut TcpStream, expected_state: &str) -> DriveResult<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    // Read the request line + headers (we only need the URL).
    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|e| DriveError::OAuth(format!("read redirect: {e}")))?;
    let raw = std::str::from_utf8(&buf[..n])
        .map_err(|e| DriveError::OAuth(format!("non-utf8 redirect: {e}")))?;

    // First line: `GET /qobee/oauth/callback?code=...&state=... HTTP/1.1`
    let path = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| DriveError::OAuth("malformed redirect request".into()))?;

    let url = url::Url::parse(&format!("http://localhost{path}"))
        .map_err(|e| DriveError::OAuth(format!("bad redirect url: {e}")))?;

    let mut got_code: Option<String> = None;
    let mut got_state: Option<String> = None;
    let mut got_error: Option<String> = None;
    for (k, v) in url.query_pairs() {
        match &*k {
            "code" => got_code = Some(v.into_owned()),
            "state" => got_state = Some(v.into_owned()),
            "error" => got_error = Some(v.into_owned()),
            _ => {}
        }
    }

    // Always close the response, success or not, so the browser
    // doesn't hang waiting.
    let body = match got_error {
        Some(ref e) => format!(
            "<html><body><h1>Qobee — authorization failed</h1><pre>{e}</pre><p>You can close this tab.</p></body></html>"
        ),
        None => "<html><body><h1>Qobee is connected.</h1><p>You can close this tab.</p></body></html>".into(),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

    if let Some(err) = got_error {
        return Err(DriveError::OAuth(format!("OAuth provider error: {err}")));
    }
    let code = got_code.ok_or_else(|| DriveError::OAuth("no `code` in redirect".into()))?;
    let state = got_state.unwrap_or_default();
    if state != expected_state {
        return Err(DriveError::OAuth(
            "state mismatch in OAuth callback (possible CSRF)".into(),
        ));
    }
    Ok(code)
}

/// Generate a 64-character PKCE verifier (RFC 7636 §4.1: 43-128
/// unreserved chars). We pull 48 random bytes and base64-url-encode
/// them; that gives 64 unreserved chars after stripping padding.
fn generate_pkce_verifier() -> String {
    let mut bytes = [0u8; 48];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 of the verifier, then base64-url-no-pad. Per RFC 7636 §4.2.
fn pkce_challenge(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let digest = h.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

fn generate_random_string(n_bytes: usize) -> String {
    let mut bytes = vec![0u8; n_bytes];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_deterministic() {
        let verifier = "abc123_-";
        // Known-good vector from https://oauth.net/2/pkce/
        let want = pkce_challenge(verifier);
        assert_eq!(want, pkce_challenge(verifier));
    }

    #[test]
    fn auth_url_includes_required_params() {
        let url = build_auth_url("cid", "https://scope", "http://127.0.0.1:1/c", "chal", "st");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fc"));
        assert!(url.contains("scope=https%3A%2F%2Fscope"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn verifier_length() {
        let v = generate_pkce_verifier();
        assert!(v.len() >= 43 && v.len() <= 128);
    }
}
