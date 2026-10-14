// SPDX-License-Identifier: MPL-2.0

//! The installed-app OAuth flow: PKCE, a loopback redirect, the Google Picker,
//! and token refresh.
//!
//! Google's desktop Picker is not a separate REST endpoint — it is triggered
//! from the authorization URL itself (`trigger_onepick=true&prompt=consent`),
//! and the chosen file ids come back on the same redirect as the authorization
//! code. So one browser trip signs the user in *and* selects documents.
//!
//! The `oauth2` crate supplies the strongly-typed flow (PKCE challenge, `state`
//! verification, code and refresh exchange). Two things stay ours because the
//! crate deliberately does not do them: the shared **loopback listener** catches
//! Google's redirect, and the shared credential port puts only refresh tokens
//! in the OS vault ([`crate::tokens`]). This module runs on the import worker thread —
//! never the window loop, which must not wait on a browser or the network.
//!
//! ## The listener is loopback-only, and that is what keeps macOS quiet
//!
//! The bind is `127.0.0.1:0` — the loopback address explicitly, and an ephemeral
//! port because Google allows any loopback port for a *Desktop app* client. Both
//! halves matter on a Mac. The macOS application firewall asks *"do you want the
//! application X to accept incoming network connections?"* when a program starts
//! listening on an interface it filters, and it does not filter loopback: even
//! *Block all incoming connections* leaves 127.0.0.1 alone. Binding `0.0.0.0`
//! would put that prompt in the middle of a consent screen, where the person is
//! looking at their browser rather than at Koru. So the address is not a
//! detail — do not "simplify" it to `localhost:0`, which resolves to whatever
//! the host file says and may reach `::1` or a real interface.
//!
//! The browser opener is injected ([`AuthConfig::open`]) and this module links
//! no browser crate; see the crate docs for why.

use std::sync::Arc;
use std::time::{Duration, Instant};

use koru_oauth_credentials::SecretString;
use koru_oauth_loopback::{CallbackListener, WaitOptions};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    RefreshToken, Scope, TokenResponse, TokenUrl,
};

use crate::client::{
    AUTH_URL, DRIVE_FILE_SCOPE, GOOGLE_DOCS_MIME, REDIRECT_PATH, Registration, TOKEN_URL,
};
use crate::tokens::TokenStore;

/// Refresh a token this far before it expires, so it cannot die mid-flight.
const EXPIRY_SLACK: Duration = Duration::from_secs(60);
/// How long to wait for the browser to land on the loopback listener before
/// giving up and telling the user something went wrong.
///
/// Five minutes is a person's budget, not a machine's: a password, a second
/// factor and a Picker full of documents all happen inside it. Nothing is
/// blocked meanwhile except this worker thread — and a cancelled import stops
/// the wait at the next poll rather than serving out the timeout.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
/// How often the wait wakes to check the deadline and the cancellation flag.

/// The OAuth half of the Google integration. [`GoogleAuth::authorize_with_picker`]
/// signs the user in and selects documents in one browser trip;
/// [`GoogleAuth::access_token`] refreshes from the keychain for the Drive calls
/// that follow.
pub struct GoogleAuth {
    registration: Registration,
    http: reqwest::blocking::Client,
    tokens: Arc<dyn TokenStore>,
    /// The last token this process minted or refreshed, cached so a burst of
    /// export calls does not re-read the keychain each time.
    access: Option<AccessToken>,
    /// Opens the system browser. A field, not a hard dependency, so a test can
    /// capture the URL instead of launching one.
    open: fn(&str) -> std::io::Result<()>,
}

pub struct AuthConfig {
    /// The Desktop app OAuth client this build was configured with
    /// ([`Registration::configured`]); never a literal in source.
    pub registration: Registration,
    pub tokens: Arc<dyn TokenStore>,
    /// How this process hands a URI to the desktop. The host supplies it —
    /// `koru_window::open_uri` in the shell — so this crate links no browser.
    pub open: fn(&str) -> std::io::Result<()>,
}

/// What one browser trip yields: a bearer token and the documents the user
/// picked while consenting.
pub struct PickerResult {
    pub access_token: String,
    pub picked_file_ids: Vec<String>,
}

/// Whether a URL is safe to hand to the desktop from this flow.
///
/// The consent URL is built here from [`crate::client::AUTH_URL`] and the
/// PKCE/state values this process minted — it is not remote input, which is why
/// this crate does not reach for `koru_editor::browser`'s allow-list (and could
/// not: the shell does not depend on the editor). The check is one line and
/// costs nothing, and it means the injected opener can never be handed anything
/// but an `https:` URL no matter what a future parameter change does to the URL
/// builder.
fn is_consent_url(url: &str) -> bool {
    url.starts_with("https://")
}

impl GoogleAuth {
    pub fn new(config: AuthConfig) -> Self {
        // Following redirects would open the token endpoint up to SSRF; the
        // oauth2 docs call this out explicitly, and no part of this flow wants
        // a redirect to be followed silently.
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a blocking HTTP client always builds");
        Self {
            registration: config.registration,
            http,
            tokens: config.tokens,
            access: None,
            open: config.open,
        }
    }

    /// The bearer token for Drive calls, refreshing from the keychain when the
    /// cached token is stale. A prior refresh credential may also establish the
    /// session; access tokens themselves are never read from durable storage.
    pub fn access_token(&mut self) -> Result<String, String> {
        if let Some(token) = &self.access
            && !expired(token)
        {
            return Ok(token.access_token.clone());
        }
        if let Some(refresh) = self.tokens.load()? {
            let refreshed = self.refresh(refresh.expose())?;
            self.access = Some(refreshed.clone());
            return Ok(refreshed.access_token);
        }
        Err("not connected to Google — sign in first".into())
    }

    /// Sign in and run the one-pick Picker in a single browser trip, then
    /// persist the token and return it alongside the picked document ids.
    ///
    /// `cancelled` is polled while waiting for the browser to come back, so an
    /// import the person abandoned — or one whose browser never opened — ends
    /// within a poll instead of holding this thread for the whole timeout. It is
    /// a closure rather than a stored flag because the authority on "was this
    /// cancelled" is the import's own `ImportProgress`, and this crate must not
    /// keep a second copy of that answer.
    pub fn authorize_with_picker(
        &mut self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PickerResult, String> {
        // Loopback, explicitly, and an ephemeral port — see the module docs for
        // what that buys on macOS.
        let listener = CallbackListener::bind(REDIRECT_PATH).map_err(|error| error.to_string())?;
        let port = listener.local_addr().port();
        let redirect = listener.redirect_uri();
        crate::debug("loopback", &format!("listening on 127.0.0.1:{port}"));

        let client = BasicClient::new(ClientId::new(self.registration.client_id.clone()))
            .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).expect("a static auth URL parses"))
            .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).expect("a static token URL parses"))
            .set_client_secret(ClientSecret::new(self.registration.client_secret.clone()))
            .set_redirect_uri(RedirectUrl::new(redirect).map_err(|error| error.to_string())?);

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (auth_url, csrf) = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new(DRIVE_FILE_SCOPE.to_string()))
            .set_pkce_challenge(challenge)
            // `prompt=consent` + `trigger_onepick=true` turn the consent screen
            // into the Picker; `allow_multiple` lets the user take several
            // documents at once and `mimetypes` keeps it to Google Docs.
            .add_extra_param("prompt", "consent")
            .add_extra_param("trigger_onepick", "true")
            .add_extra_param("allow_multiple", "true")
            .add_extra_param("mimetypes", GOOGLE_DOCS_MIME)
            .url();

        if !is_consent_url(auth_url.as_str()) {
            return Err("refusing to open a consent URL that is not https".into());
        }
        (self.open)(auth_url.as_str())
            .map_err(|error| format!("could not open your browser: {error}"))?;

        let callback = wait_for_callback(listener, csrf.secret(), cancelled)?;

        let token = client
            .exchange_code(AuthorizationCode::new(callback.code))
            .set_pkce_verifier(verifier)
            .request(&self.http)
            .map_err(|error| format!("Google rejected the authorization: {error}"))?;

        let access = token_to_access(&token)?;
        if let Some(refresh) = token.refresh_token() {
            self.tokens.store(
                SecretString::new(refresh.secret().clone()).map_err(|error| error.to_string())?,
            )?;
        }
        self.access = Some(access.clone());
        Ok(PickerResult {
            access_token: access.access_token,
            picked_file_ids: callback.picked_file_ids,
        })
    }

    /// Exchange a refresh token for a fresh access token and persist the result
    /// (Google may rotate the refresh token in the process).
    fn refresh(&self, refresh_token: &str) -> Result<AccessToken, String> {
        // A refresh exchange carries no redirect URI; the placeholder only
        // satisfies the client's typestate and is never sent.
        let client = BasicClient::new(ClientId::new(self.registration.client_id.clone()))
            .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).expect("a static auth URL parses"))
            .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).expect("a static token URL parses"))
            .set_client_secret(ClientSecret::new(self.registration.client_secret.clone()))
            .set_redirect_uri(
                RedirectUrl::new(format!("http://127.0.0.1:0{REDIRECT_PATH}"))
                    .expect("a static loopback URL parses"),
            );
        let token = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request(&self.http)
            .map_err(|error| format!("Google rejected the token refresh: {error}"))?;
        // Google normally omits a refresh token here. Keep the existing
        // keychain value unless it explicitly rotates one.
        if let Some(rotated) = token.refresh_token() {
            self.tokens.store(
                SecretString::new(rotated.secret().clone()).map_err(|error| error.to_string())?,
            )?;
        }
        token_to_access(&token)
    }
}

#[derive(Clone)]
struct AccessToken {
    access_token: String,
    expiry: Expiry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expiry {
    Unspecified,
    At(Instant),
}

fn token_to_access(token: &oauth2::basic::BasicTokenResponse) -> Result<AccessToken, String> {
    access_token_from(
        token.access_token().secret().clone(),
        token.expires_in(),
        Instant::now(),
    )
}

fn access_token_from(
    access_token: String,
    lifetime: Option<Duration>,
    now: Instant,
) -> Result<AccessToken, String> {
    let expiry = match lifetime {
        None => Expiry::Unspecified,
        Some(lifetime) => Expiry::At(now.checked_add(lifetime).ok_or_else(|| {
            "Google returned a token lifetime outside the supported range".to_string()
        })?),
    };
    Ok(AccessToken {
        access_token,
        expiry,
    })
}

fn expired(token: &AccessToken) -> bool {
    expired_at(token, Instant::now())
}

fn expired_at(token: &AccessToken, now: Instant) -> bool {
    match token.expiry {
        Expiry::Unspecified => false,
        Expiry::At(expiry) => now
            .checked_add(EXPIRY_SLACK)
            .is_none_or(|threshold| expiry <= threshold),
    }
}

/// What the loopback redirect carried: the authorization code and the file ids
/// the user picked (empty when nothing was selected).
struct Callback {
    code: String,
    picked_file_ids: Vec<String>,
}

/// Accept exactly one connection, read `code`, `state` and `picked_file_ids`
/// from the query string, verify `state`, answer with a page, and return the
/// parsed callback. The `state` check is the whole CSRF defence: an attacker
/// who somehow redirects the browser here does not know the value this process
/// minted.
fn wait_for_callback(
    listener: CallbackListener,
    expected_state: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Callback, String> {
    let success = success_page();
    let error = error_page();
    let callback = listener
        .wait(
            &WaitOptions {
                expected_state,
                timeout: CALLBACK_TIMEOUT,
                poll: koru_oauth_loopback::DEFAULT_POLL,
                max_request_bytes: koru_oauth_loopback::DEFAULT_MAX_REQUEST_BYTES,
                success_html: &success,
                error_html: &error,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let code = callback
        .value("code")
        .ok_or_else(|| "Google's redirect did not carry an authorization code".to_string())?
        .to_owned();
    let picked_file_ids = callback
        .value("picked_file_ids")
        .into_iter()
        .flat_map(|ids| ids.split(','))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Callback {
        code,
        picked_file_ids,
    })
}

/// The page shown after a successful sign-in: a 3, 2, 1 countdown, then
/// `window.close()`. Browsers only honour that for tabs opened by the flow, so
/// the closing text doubles as a fallback instruction.
fn success_page() -> String {
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Koru</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;padding-top:3rem">
<p>You can close this tab and return to Koru.</p>
<p id="n" style="font-size:2rem">3</p>
<script>
var n = 3;
var el = document.getElementById("n");
var t = setInterval(function () {
  n = n - 1;
  if (n <= 0) {
    clearInterval(t);
    window.close();
    el.textContent = "";
  } else {
    el.textContent = n;
  }
}, 1000);
</script>
</body></html>"#
        .to_string()
}

fn error_page() -> String {
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Koru</title></head>\
     <body style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:3rem\">\
     <p>Sign-in was not completed. You can close this tab and try again.</p>\
     </body></html>"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fake_open(_url: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Drive one callback through the loopback listener and read the result back.
    fn round_trip_callback(state: &str, sent_state: &str, query: &str) -> Result<Callback, String> {
        let listener = CallbackListener::bind(REDIRECT_PATH).unwrap();
        let port = listener.local_addr().port();
        let request = format!(
            "GET {REDIRECT_PATH}?{query}&state={sent_state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        );
        let handle = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
        });
        let result = wait_for_callback(listener, state, &|| false);
        handle.join().unwrap();
        result
    }

    #[test]
    fn the_callback_returns_the_code_and_picked_ids_when_state_matches() {
        let callback =
            round_trip_callback("s", "s", "code=abc-123&picked_file_ids=idA,idB").unwrap();
        assert_eq!(callback.code, "abc-123");
        assert_eq!(callback.picked_file_ids, ["idA", "idB"]);
    }

    #[test]
    fn a_percent_encoded_code_is_decoded_before_the_exchange() {
        let callback = round_trip_callback("s", "s", "code=4%2F0AanRRrs").unwrap();
        assert_eq!(callback.code, "4/0AanRRrs");
    }

    #[test]
    fn a_callback_without_picked_files_yields_an_empty_list() {
        let callback = round_trip_callback("s", "s", "code=abc-123").unwrap();
        assert!(callback.picked_file_ids.is_empty());
    }

    #[test]
    fn the_callback_rejects_a_mismatched_state() {
        assert!(round_trip_callback("expected", "attacker", "code=abc-123").is_err());
    }

    #[test]
    fn a_token_with_no_expiry_is_never_expired() {
        let token = AccessToken {
            access_token: "a".into(),
            expiry: Expiry::Unspecified,
        };
        assert!(!expired(&token));
    }

    #[test]
    fn a_token_expiring_inside_the_slack_is_expired() {
        let now = Instant::now();
        let token = AccessToken {
            access_token: "a".into(),
            expiry: Expiry::At(now.checked_add(Duration::from_secs(30)).unwrap()),
        };
        assert!(expired_at(&token, now));
    }

    #[test]
    fn an_unrepresentable_provider_lifetime_is_unusable() {
        let now = Instant::now();
        assert!(access_token_from("a".into(), Some(Duration::MAX), now).is_err());
    }

    #[test]
    fn an_injected_opener_is_what_the_flow_holds() {
        // The opener is a plain function pointer, supplied by the host; this
        // pins that construction takes one and that nothing is minted yet.
        let auth = GoogleAuth::new(AuthConfig {
            registration: Registration::from_values(Some("x"), Some("y")).unwrap(),
            tokens: Arc::new(crate::tokens::SessionTokenStore::default()),
            open: fake_open,
        });
        assert!(auth.access.is_none());
    }

    /// The consent URL this flow builds is always Google's `https:` endpoint,
    /// and the opener is never handed anything else.
    #[test]
    fn only_an_https_url_reaches_the_opener() {
        assert!(is_consent_url(crate::client::AUTH_URL));
        for url in [
            "http://accounts.google.com/o/oauth2/v2/auth",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!is_consent_url(url), "{url} should not be opened");
        }
    }

    /// A cancelled import must not hold the worker thread for the whole
    /// five-minute timeout. Nothing connects here; the wait ends because the
    /// flag says so.
    #[test]
    fn a_cancelled_wait_gives_up_at_the_next_poll() {
        let listener = CallbackListener::bind(REDIRECT_PATH).unwrap();
        let started = std::time::Instant::now();
        let result = wait_for_callback(listener, "s", &|| true);
        assert!(result.is_err(), "a cancelled wait returns an error");
        assert!(
            started.elapsed() < CALLBACK_TIMEOUT,
            "the wait served out its timeout instead of reading the flag"
        );
    }

    /// The listener is loopback-only: the address is what keeps the macOS
    /// firewall out of the middle of a consent screen.
    #[test]
    fn the_callback_listener_binds_loopback_only() {
        let listener = CallbackListener::bind(REDIRECT_PATH).unwrap();
        let addr = listener.local_addr();
        assert!(
            addr.ip().is_loopback(),
            "bound {addr}, which is not loopback"
        );
        assert_ne!(addr.port(), 0, "an ephemeral port is chosen by the OS");
    }
}
