//! The portal's API: `POST <api base>/Portal/<area>/<call>` with a JSON body,
//! answered with an envelope (`model::Envelope`).
//!
//! A session is two values the portal's web app keeps in its local storage
//! and sends as headers on every call: `duuid`, a random id the app makes
//! once per device, and `suuid`, the session id the sign-in answers
//! (`uniqueSessionID`). The portal binds the session to the device id: the
//! right session with another device id is refused ("Existing session not
//! found"), and the session stays good. `docs/api.md` has the endpoints and
//! their traps.
//!
//! Each call is one request, never retried: a sign-in that fails is not
//! sent again, so a wrong password is spent once.

use std::time::Duration;

use pk_cli_core::{transport, CliError, Reason};
use pk_cli_secrets::Secret;
use serde_json::{json, Value};

use crate::cli::BIN;
use crate::model::{self, Envelope, LoginAnswer, PortalConfig};

/// The `co` header the web app sends on every call, always this value.
const CO: &str = "1901-01-01T00:00:00Z";
const TIMEOUT: Duration = Duration::from_secs(60);

/// Every API path fertemr may send, by its last segment or whole: the
/// portal's reads. Writes (sending a message, marking it read, saving a
/// form, accepting terms) are refused before a request is made.
const READS: &[&str] = &[
    "List",
    "GetByID",
    "InitialData",
    "Instance",
    "ListInstances",
    "ListTemplates",
    "GetTemplate",
    "DownloadFile",
    "DownloadAttachment",
    "DownloadCalEvent",
];

/// A portal's session, as stored (`session::Session`) and sent.
pub struct Device {
    pub duuid: String,
    pub suuid: Secret,
}

/// A clinic's portal, by its web address (`https://portal-<clinic>.fertemr.com`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalAddress {
    /// Scheme and host (and port), no trailing slash.
    pub origin: String,
    /// The short name: `<clinic>` of `portal-<clinic>.fertemr.com`, else the
    /// host. The organization's id.
    pub slug: String,
}

impl PortalAddress {
    /// A portal from its short name, host or URL. Only `https`, or `http` to
    /// this machine (a test's stub).
    pub fn parse(input: &str) -> Result<PortalAddress, CliError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(CliError::Usage("the portal is empty".into()));
        }
        let url = if input.contains("://") {
            input.to_string()
        } else if input.contains('.') || input.contains(':') {
            format!("https://{input}")
        } else {
            if !input.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(CliError::Usage(format!(
                    "`{input}` is not a portal: give its host (portal-<clinic>.fertemr.com), its URL or <clinic>"
                )));
            }
            format!("https://portal-{}.fertemr.com", input.to_ascii_lowercase())
        };
        transport::require_secure_url(&url)?;
        let (scheme, rest) = url.split_once("://").expect("checked above");
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if authority.is_empty() || authority.contains('@') {
            return Err(CliError::Usage("the portal's address has no host".into()));
        }
        let host = authority.split(':').next().unwrap_or_default().to_string();
        let slug = host
            .strip_prefix("portal-")
            .and_then(|h| h.strip_suffix(".fertemr.com"))
            .filter(|s| !s.is_empty() && !s.contains('.'))
            .map(str::to_string)
            .unwrap_or_else(|| authority.replace(':', "-"));
        Ok(PortalAddress {
            origin: format!("{}://{authority}", scheme.to_ascii_lowercase()),
            slug,
        })
    }

    fn host(&self) -> &str {
        self.origin.split_once("://").map_or("", |(_, h)| h)
    }

    /// Whether `url` is an API base this portal may name: `https` on a
    /// fertemr.com host, or a test stub on the portal's own local host.
    pub fn accepts_api_base(&self, url: &str) -> bool {
        let Some((scheme, rest)) = url.split_once("://") else {
            return false;
        };
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if authority.contains('@') {
            return false;
        }
        let host = authority.split(':').next().unwrap_or_default();
        match scheme.to_ascii_lowercase().as_str() {
            "https" => host.ends_with(".fertemr.com") || authority == self.host(),
            "http" => authority == self.host() && transport::require_secure_url(url).is_ok(),
            _ => false,
        }
    }
}

/// What a portal's public config.json says about it.
#[derive(Debug, Clone)]
pub struct PortalInfo {
    pub address: PortalAddress,
    /// The API base: no trailing slash.
    pub api_base: String,
    pub client_code: Option<String>,
    pub name: String,
    pub environment: Option<String>,
}

fn client() -> Result<reqwest::blocking::Client, CliError> {
    reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .redirect(transport::secure_redirects())
        .user_agent(format!("{BIN}/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| CliError::Other(format!("building the HTTP client: {e}")))
}

/// The portal's public configuration: one GET, no credential.
pub fn discover(address: &PortalAddress) -> Result<PortalInfo, CliError> {
    let url = format!("{}/config.json", address.origin);
    let resp = client()?.get(&url).send().map_err(|e| net_error(&e))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Err(CliError::NotFound(format!(
            "{} has no portal configuration (config.json answered 404): is it a FERT portal?",
            address.origin
        )));
    }
    if !(200..300).contains(&status) {
        return Err(CliError::Upstream(format!(
            "{}/config.json answered HTTP {status}",
            address.origin
        )));
    }
    let body: Value = resp
        .json()
        .map_err(|_| CliError::Upstream(format!("{}/config.json is not JSON", address.origin)))?;
    let cfg: PortalConfig = model::read(body, "config.json")?;
    let Some(base) = cfg.base_url else {
        return Err(CliError::Upstream(format!(
            "{}/config.json names no API (baseUrl)",
            address.origin
        )));
    };
    let base = base.trim_end_matches('/').to_string();
    if !address.accepts_api_base(&base) {
        return Err(CliError::Upstream(format!(
            "{}/config.json names an API that is not on fertemr.com over https; fertemr sends no sign-in there",
            address.origin
        )));
    }
    Ok(PortalInfo {
        address: address.clone(),
        api_base: base,
        name: cfg.name_full.or(cfg.name_short).unwrap_or_else(|| address.slug.clone()),
        client_code: cfg.client_code,
        environment: cfg.environment,
    })
}

/// The portal's API at `base`.
pub struct Api {
    base: String,
    http: reqwest::blocking::Client,
}

/// What the portal answered for a path that is not one of its reads.
pub fn check_read(path: &str) -> Result<String, CliError> {
    let p = path.trim().trim_start_matches('/');
    let p = p.strip_prefix("Portal/").unwrap_or(p);
    let ok_chars = !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '/');
    let last = p.rsplit('/').next().unwrap_or_default();
    if !ok_chars || !p.contains('/') || !READS.iter().any(|r| last.eq_ignore_ascii_case(r)) {
        return Err(CliError::Usage(format!(
            "`{p}` is not one of the portal's reads; fertemr sends only paths ending in {}",
            READS.join(", ")
        )));
    }
    Ok(p.to_string())
}

impl Api {
    pub fn new(base: &str) -> Result<Api, CliError> {
        transport::require_secure_url(base)?;
        Ok(Api {
            base: base.trim_end_matches('/').to_string(),
            http: client()?,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn request(&self, path: &str, device_id: &str, session: &str, body: &Value) -> reqwest::blocking::RequestBuilder {
        self.http
            .post(format!("{}/Portal/{path}", self.base))
            .header("content-type", "application/json; charset=UTF-8")
            .header("duuid", device_id)
            .header("suuid", session)
            .header("co", CO)
            .body(body.to_string())
    }

    /// `User/LoginAttempt`: a new session for `device_id`.
    pub fn sign_in(&self, device_id: &str, username: &Secret, password: &Secret) -> Result<LoginAnswer, CliError> {
        let os = json!({ "appName": BIN, "appVersion": env!("CARGO_PKG_VERSION") }).to_string();
        let body = json!({
            "username": username.expose(),
            "password": password.expose(),
            "osDetail": os,
            "pushToken": "",
        });
        let resp = self
            .request("User/LoginAttempt", device_id, "", &body)
            .send()
            .map_err(|e| net_error(&e))?;
        login_answer(resp, "the sign-in")
    }

    /// `User/GetUserPrefs`, a GET: the signed-in user, as the app reads it
    /// on opening, in the envelope every read but the sign-in comes in. A
    /// read with no effect, so it is how fertemr checks a session (`auth
    /// status --check`, a `--browser` sign-in).
    pub fn current_user(&self, device: &Device) -> Result<LoginAnswer, CliError> {
        let resp = self
            .http
            .get(format!("{}/Portal/User/GetUserPrefs", self.base))
            .header("content-type", "application/json")
            .header("duuid", &device.duuid)
            .header("suuid", device.suuid.expose())
            .header("co", CO)
            .send()
            .map_err(|e| net_error(&e))?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(|e| net_error(&e))?;
        current_user_answer(status, &text)
    }

    /// `User/Logout`, a GET in the web app.
    pub fn sign_out(&self, device: &Device) -> Result<(), CliError> {
        let resp = self
            .http
            .get(format!("{}/Portal/User/Logout", self.base))
            .header("duuid", &device.duuid)
            .header("suuid", device.suuid.expose())
            .header("co", CO)
            .send()
            .map_err(|e| net_error(&e))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        // A session the portal already dropped answers 500 with an envelope
        // saying so: signed out either way. Any other failure is not.
        let text = resp.text().unwrap_or_default();
        let said = serde_json::from_str::<Envelope>(&text)
            .ok()
            .and_then(|e| e.t)
            .unwrap_or_default();
        if session_gone(&said) {
            return Ok(());
        }
        Err(CliError::Upstream(format!("the sign-out answered HTTP {status}")))
    }

    /// One read, its envelope's `d`. `path` must pass [`check_read`].
    pub fn read(&self, device: &Device, path: &str, body: &Value) -> Result<Value, CliError> {
        let path = check_read(path)?;
        let resp = self
            .request(&path, &device.duuid, device.suuid.expose(), body)
            .send()
            .map_err(|e| net_error(&e))?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(|e| net_error(&e))?;
        let env: Envelope = serde_json::from_str(&text).map_err(|_| not_json(&path, status, &text))?;
        envelope(&path, status, env)
    }

    /// One download: the file's bytes, or the envelope's error.
    pub fn download(&self, device: &Device, path: &str, body: &Value) -> Result<Vec<u8>, CliError> {
        let path = check_read(path)?;
        let resp = self
            .request(&path, &device.duuid, device.suuid.expose(), body)
            .send()
            .map_err(|e| net_error(&e))?;
        let status = resp.status().as_u16();
        let json = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("json"));
        let bytes = resp.bytes().map_err(|e| net_error(&e))?.to_vec();
        if json {
            // A file is never JSON here: this is an envelope saying why not.
            let env: Envelope = serde_json::from_slice(&bytes).map_err(|_| {
                CliError::Upstream(format!(
                    "{path} answered HTTP {status} with JSON that is not an envelope"
                ))
            })?;
            envelope(&path, status, env)?;
            return Err(CliError::Upstream(format!(
                "{path} answered an envelope instead of a file"
            )));
        }
        if !(200..300).contains(&status) {
            return Err(CliError::Upstream(format!("{path} answered HTTP {status}")));
        }
        Ok(bytes)
    }
}

/// The portal's error texts that mean the session is gone.
fn session_gone(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("session could not be found") || t.contains("existing session not found") || t.contains("please log in")
}

fn envelope(path: &str, status: u16, env: Envelope) -> Result<Value, CliError> {
    if env.c == Some(0) && (200..300).contains(&status) {
        return Ok(env.d);
    }
    let text = env.t.unwrap_or_default();
    if session_gone(&text) {
        return Err(signed_out());
    }
    if text.to_ascii_lowercase().contains("requested resource was not found") {
        return Err(CliError::NotFound(format!(
            "{path}: the portal does not offer it to this patient"
        )));
    }
    Err(CliError::Upstream(format!(
        "{path} answered HTTP {status}, code {}{}",
        env.c.map_or("none".to_string(), |c| c.to_string()),
        if text.is_empty() {
            String::new()
        } else {
            format!(": {}", one_line(&text))
        }
    )))
}

/// The session ended at the portal: exit 3, sign in again.
pub fn signed_out() -> CliError {
    CliError::Auth("the portal no longer accepts the stored session".into())
        .because(Reason::CredentialRefused)
        .with_remedy(format!("{BIN} auth login --profile <NAME> signs in again"))
}

fn login_answer(resp: reqwest::blocking::Response, what: &str) -> Result<LoginAnswer, CliError> {
    let status = resp.status().as_u16();
    let text = resp.text().map_err(|e| net_error(&e))?;
    let answer: LoginAnswer =
        serde_json::from_str(&text).map_err(|_| not_json(&format!("{what} (User)"), status, &text))?;
    if answer.success == Some(true) && answer.session_id.is_some() {
        return Ok(answer);
    }
    let said = answer
        .err
        .clone()
        .or(answer.t.clone())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let lower = said.to_ascii_lowercase();
    if lower.contains("user name or password") || lower.contains("incorrect") || session_gone(&said) {
        return Err(
            CliError::Auth(format!("{what} was refused: {}", one_line(&said))).because(Reason::CredentialRefused)
        );
    }
    if lower.contains("code") || lower.contains("two-factor") || lower.contains("verification") {
        return Err(CliError::Auth(format!(
            "{what} asks for a verification code: {}",
            one_line(&said)
        ))
        .because(Reason::CredentialRefused)
        .with_remedy(format!("sign in with `{BIN} auth login --browser --profile <NAME>`, where the code is entered on the portal's page")));
    }
    if (500..600).contains(&status) && answer.c.is_none() {
        return Err(CliError::Upstream(format!("{what} answered HTTP {status}")));
    }
    Err(CliError::Auth(format!("{what} did not succeed: {}", one_line(&said))).because(Reason::CredentialRefused))
}

/// The signed-in user from `User/GetUserPrefs`' answer: the envelope's `d`,
/// which must be a user. An ended session is exit 3; anything else that is
/// not a user is an upstream error, never a live session.
fn current_user_answer(status: u16, text: &str) -> Result<LoginAnswer, CliError> {
    const PATH: &str = "User/GetUserPrefs";
    if status == 401 || status == 403 {
        return Err(signed_out());
    }
    let env: Envelope = serde_json::from_str(text).map_err(|_| not_json(PATH, status, text))?;
    let user = envelope(PATH, status, env)?;
    // The error names no value: the answer is the patient's own.
    let not_a_user = || CliError::Upstream(format!("{PATH} answered HTTP {status} with no user in it"));
    if !user.is_object() {
        return Err(not_a_user());
    }
    serde_json::from_value(user).map_err(|_| not_a_user())
}

fn not_json(path: &str, status: u16, text: &str) -> CliError {
    let looks_html = text.trim_start().starts_with('<');
    CliError::Upstream(format!(
        "{path} answered HTTP {status} with {}",
        if looks_html {
            "a web page, not JSON"
        } else {
            "something that is not JSON"
        }
    ))
}

fn net_error(e: &reqwest::Error) -> CliError {
    // reqwest's message can carry the URL; the host is all that is said.
    let host = e.url().and_then(|u| u.host_str()).unwrap_or("the portal").to_string();
    let what = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_redirect() {
        "was redirected somewhere fertemr does not follow"
    } else {
        "failed"
    };
    CliError::Upstream(format!("the request to {host} {what}"))
}

/// One line, at most 200 characters, of a portal message.
fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 200 {
        format!("{}…", flat.chars().take(200).collect::<String>())
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_current_user_is_the_envelopes_user_and_nothing_else() {
        use super::current_user_answer as read;
        let live = include_str!("../tests/fixtures/portal/user.json");
        assert_eq!(read(200, live).unwrap().pt_id, Some(303));
        let gone = include_str!("../tests/fixtures/portal/session-gone.json");
        for (status, text, auth) in [
            (500, gone, true),
            (401, "", true),
            (403, "", true),
            (200, r#"{"c":0,"t":"","d":null,"r":null}"#, false),
            (200, r#"{"c":0,"t":"","d":"a user","r":null}"#, false),
            (200, r#"{"c":0,"t":"","d":[],"r":null}"#, false),
            (500, r#"{"c":9,"t":"Something went wrong","d":null,"r":null}"#, false),
            (200, "<html>sign in</html>", false),
        ] {
            let e = read(status, text).expect_err(text);
            assert_eq!(e.is_auth(), auth, "{status} {text}: {}", e.message());
            assert!(auth || e.exit_code() == 5, "{status} {text}: {}", e.message());
        }
    }

    use super::*;

    #[test]
    fn a_portal_from_its_short_name_host_or_url() {
        let want = PortalAddress {
            origin: "https://portal-someclinic.fertemr.com".into(),
            slug: "someclinic".into(),
        };
        assert_eq!(PortalAddress::parse("someclinic").unwrap(), want);
        assert_eq!(PortalAddress::parse("portal-someclinic.fertemr.com").unwrap(), want);
        assert_eq!(
            PortalAddress::parse("https://Portal-SomeClinic.fertemr.com/#/Login").unwrap(),
            want
        );
        let local = PortalAddress::parse("http://127.0.0.1:9").unwrap();
        assert_eq!(local.origin, "http://127.0.0.1:9");
        assert_eq!(local.slug, "127.0.0.1-9");
        assert!(PortalAddress::parse("http://portal-x.fertemr.com").is_err());
        assert!(PortalAddress::parse("some clinic").is_err());
        // Built, so the personal-data scan does not read it as an address.
        let with_user = format!("https://user{}portal-x.fertemr.com", '@');
        assert!(PortalAddress::parse(&with_user).is_err());
    }

    #[test]
    fn only_a_fertemr_api_gets_a_sign_in() {
        let p = PortalAddress::parse("someclinic").unwrap();
        assert!(p.accepts_api_base("https://portal-someclinic-api.fertemr.com"));
        assert!(!p.accepts_api_base("http://portal-someclinic-api.fertemr.com"));
        assert!(!p.accepts_api_base("https://evil.example/fertemr.com"));
        assert!(!p.accepts_api_base("https://fertemr.com.evil.example"));
        let local = PortalAddress::parse("http://127.0.0.1:4545").unwrap();
        assert!(local.accepts_api_base("http://127.0.0.1:4545/api"));
        assert!(!local.accepts_api_base("http://127.0.0.1:9999"));
    }

    #[test]
    fn only_reads_are_sent() {
        assert_eq!(check_read("PPLab/List").unwrap(), "PPLab/List");
        assert_eq!(
            check_read("/Portal/PPFiles/DownloadFile").unwrap(),
            "PPFiles/DownloadFile"
        );
        for bad in [
            "PPMessaging/SaveOrSendMessage",
            "PPMessaging/SetReadStatus",
            "User/AcceptTOS",
            "PPHP/SaveAnswers",
            "List",
            "PPLab/List?x=1",
            "../PPLab/List",
        ] {
            assert!(check_read(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_gone_session_is_exit_3() {
        let env = Envelope {
            c: Some(2),
            t: Some("Existing session not found, please log in.".into()),
            d: Value::Null,
        };
        let e = envelope("PPLab/List", 500, env).unwrap_err();
        assert_eq!(e.exit_code(), 3);
        let env = Envelope {
            c: Some(1),
            t: Some("The requested resource was not found".into()),
            d: Value::Null,
        };
        assert_eq!(envelope("PPEmbryology/List", 500, env).unwrap_err().exit_code(), 4);
        let env = Envelope {
            c: Some(9),
            t: Some("Something else".into()),
            d: Value::Null,
        };
        assert_eq!(envelope("PPLab/List", 500, env).unwrap_err().exit_code(), 5);
    }
}
