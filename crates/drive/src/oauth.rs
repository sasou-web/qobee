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
            return Err(classify_token_error(status, body));
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
        // 403 / explicit `error=access_denied` etc. mean Google
        // refuses the user, not the token — caller routes to
        // `DriveErrorScreen` instead of replaying the wizard.
        if status == 403 {
            return Err(DriveError::AccessDenied { reason: body });
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

    let parsed = parse_redirect_query(path)?;

    // Always close the response, success or not, so the browser
    // doesn't hang waiting. Page copy is in French (R5.1) — the
    // tab is the only thing the user sees right after the consent
    // screen and Qobee is a French-speaking app.
    let body = match parsed.outcome {
        RedirectOutcome::Code(_) => "<html><body><h1>Qobee est connecté.</h1>\
             <p>Tu peux fermer cet onglet.</p></body></html>"
            .to_string(),
        RedirectOutcome::AccessDenied(ref reason) => format!(
            "<html><body><h1>Qobee — accès refusé</h1>\
             <pre>{reason}</pre>\
             <p>Tu peux fermer cet onglet.</p></body></html>"
        ),
        RedirectOutcome::Error(ref err) => format!(
            "<html><body><h1>Qobee — autorisation échouée</h1>\
             <pre>{err}</pre>\
             <p>Tu peux fermer cet onglet.</p></body></html>"
        ),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

    match parsed.outcome {
        RedirectOutcome::AccessDenied(reason) => Err(DriveError::AccessDenied { reason }),
        RedirectOutcome::Error(err) => {
            Err(DriveError::OAuth(format!("OAuth provider error: {err}")))
        }
        RedirectOutcome::Code(code) => {
            if parsed.state.as_deref().unwrap_or("") != expected_state {
                return Err(DriveError::OAuth(
                    "state mismatch in OAuth callback (possible CSRF)".into(),
                ));
            }
            Ok(code)
        }
    }
}

/// Outcome of parsing the `?code=…&state=…` (or `?error=…`) tail
/// of a redirect URL. Split out from `handle_redirect` so we can
/// unit-test it without spinning up a TCP listener.
#[derive(Debug, PartialEq, Eq)]
enum RedirectOutcome {
    /// `error=access_denied | admin_policy_enforced | unauthorized_client`
    /// — Google refused the user. Reason carries the raw `error`
    /// (and `error_description` when present).
    AccessDenied(String),
    /// Any other `error=...` value — generic OAuth failure.
    Error(String),
    /// Authorization code captured. The caller still has to verify
    /// `state` matches.
    Code(String),
}

#[derive(Debug)]
struct ParsedRedirect {
    outcome: RedirectOutcome,
    state: Option<String>,
}

/// Returns the access-denial classification for a Google `error=...`
/// query parameter. These three values are the ones Google emits
/// when the consent flow itself refuses the user (R5.1):
///
/// * `access_denied`         — the user clicked Cancel or the app is
///   not verified and the account is not in the test users list.
/// * `admin_policy_enforced` — Workspace admin policy blocks the
///   scopes / app.
/// * `unauthorized_client`   — the OAuth client is misconfigured for
///   this account (wrong type, wrong project).
fn is_access_denied(error: &str) -> bool {
    matches!(
        error,
        "access_denied" | "admin_policy_enforced" | "unauthorized_client"
    )
}

fn parse_redirect_query(path: &str) -> DriveResult<ParsedRedirect> {
    let url = url::Url::parse(&format!("http://localhost{path}"))
        .map_err(|e| DriveError::OAuth(format!("bad redirect url: {e}")))?;

    let mut got_code: Option<String> = None;
    let mut got_state: Option<String> = None;
    let mut got_error: Option<String> = None;
    let mut got_error_description: Option<String> = None;
    for (k, v) in url.query_pairs() {
        match &*k {
            "code" => got_code = Some(v.into_owned()),
            "state" => got_state = Some(v.into_owned()),
            "error" => got_error = Some(v.into_owned()),
            "error_description" => got_error_description = Some(v.into_owned()),
            _ => {}
        }
    }

    let outcome = if let Some(err) = got_error {
        let reason = match got_error_description {
            Some(desc) if !desc.is_empty() => format!("{err}: {desc}"),
            _ => err.clone(),
        };
        if is_access_denied(&err) {
            RedirectOutcome::AccessDenied(reason)
        } else {
            RedirectOutcome::Error(reason)
        }
    } else {
        let code = got_code.ok_or_else(|| DriveError::OAuth("no `code` in redirect".into()))?;
        RedirectOutcome::Code(code)
    };

    Ok(ParsedRedirect {
        outcome,
        state: got_state,
    })
}

/// Map a non-2xx response from the token endpoint to the right
/// `DriveError` variant. Mirrors the redirect classifier so a 403
/// at any point in the flow ends up on `DriveErrorScreen` instead
/// of as a raw API error.
fn classify_token_error(status: u16, body: String) -> DriveError {
    if status == 403 {
        return DriveError::AccessDenied { reason: body };
    }
    // Cheap parse: Google returns JSON like
    // `{"error":"access_denied","error_description":"..."}`.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
            if is_access_denied(err) {
                return DriveError::AccessDenied { reason: body };
            }
        }
    }
    DriveError::Api { status, body }
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

    // --- Redirect parser ---------------------------------------------------

    #[test]
    fn redirect_parses_authorization_code() {
        let parsed = parse_redirect_query("/qobee/oauth/callback?code=abc&state=st").unwrap();
        assert_eq!(parsed.outcome, RedirectOutcome::Code("abc".into()));
        assert_eq!(parsed.state.as_deref(), Some("st"));
    }

    #[test]
    fn redirect_classifies_access_denied_user_decline() {
        let parsed = parse_redirect_query(
            "/qobee/oauth/callback?error=access_denied&error_description=The%20user%20denied%20the%20request",
        )
        .unwrap();
        match parsed.outcome {
            RedirectOutcome::AccessDenied(reason) => {
                assert!(reason.contains("access_denied"));
                assert!(reason.contains("denied the request"));
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[test]
    fn redirect_classifies_admin_policy_enforced() {
        let parsed =
            parse_redirect_query("/qobee/oauth/callback?error=admin_policy_enforced").unwrap();
        assert!(matches!(parsed.outcome, RedirectOutcome::AccessDenied(_)));
    }

    #[test]
    fn redirect_classifies_unauthorized_client() {
        let parsed =
            parse_redirect_query("/qobee/oauth/callback?error=unauthorized_client").unwrap();
        assert!(matches!(parsed.outcome, RedirectOutcome::AccessDenied(_)));
    }

    #[test]
    fn redirect_keeps_unknown_errors_generic() {
        let parsed = parse_redirect_query("/qobee/oauth/callback?error=server_error").unwrap();
        assert!(matches!(parsed.outcome, RedirectOutcome::Error(_)));
    }

    #[test]
    fn redirect_missing_both_code_and_error_is_oauth_error() {
        let err = parse_redirect_query("/qobee/oauth/callback").unwrap_err();
        match err {
            DriveError::OAuth(msg) => assert!(msg.contains("no `code`")),
            other => panic!("expected OAuth error, got {other:?}"),
        }
    }

    // --- HTTP 403 classifier -----------------------------------------------

    #[test]
    fn http_403_maps_to_access_denied() {
        let err = classify_token_error(403, "{\"error\":\"forbidden\"}".into());
        match err {
            DriveError::AccessDenied { reason } => assert!(reason.contains("forbidden")),
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[test]
    fn http_400_with_access_denied_body_maps_to_access_denied() {
        let err = classify_token_error(
            400,
            "{\"error\":\"access_denied\",\"error_description\":\"App not verified\"}".into(),
        );
        assert!(matches!(err, DriveError::AccessDenied { .. }));
    }

    #[test]
    fn http_500_stays_generic_api_error() {
        let err = classify_token_error(500, "boom".into());
        match err {
            DriveError::Api { status, .. } => assert_eq!(status, 500),
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // --- Scope assertions --------------------------------------------------

    #[test]
    fn drive_scope_is_exactly_readonly_pair() {
        // R5.4: Qobee asks for `drive.readonly` and
        // `drive.metadata.readonly` and nothing else.
        let scopes: Vec<&str> = crate::DRIVE_SCOPE.split_whitespace().collect();
        assert_eq!(
            scopes,
            vec![
                "https://www.googleapis.com/auth/drive.readonly",
                "https://www.googleapis.com/auth/drive.metadata.readonly",
            ]
        );
        assert!(!crate::DRIVE_SCOPE.contains("drive.file"));
        assert!(!crate::DRIVE_SCOPE.contains("/drive "));
    }
}
