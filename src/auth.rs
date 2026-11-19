//! `auth login`, `auth status`, `auth logout`, `auth reset`, and what
//! `profiles remove` removes: a portal session per profile, kept through
//! [`SessionStore`].
//!
//! The sign-in sends the user id and password once, to the API the portal's
//! public config.json names, and keeps only the session the portal answers.
//! The password is never stored: when the portal ends the session, the
//! owner signs in again.
//!
//! A `--browser` sign-in runs Chrome in the profile's own kept browser
//! profile, so one account's window never opens signed in as another, and
//! reads the session the portal's page keeps in its local storage. In a
//! hermetic run ([`Env::hermetic`]: `FERTEMR_KEYCHAIN_SERVICE` set, or a
//! test), nothing touches the real browser profiles.

use pk_cli_auth::{AuthLogin, AuthMethod, AuthStatus};
use pk_cli_browser::{BrowserError, Capture, HostRule, Launch};
use pk_cli_config::profile::ProfileName;
use pk_cli_core::{output, CliError, Reason};
use pk_cli_secrets::{OnePassword, OpRef, Secret};
use serde_json::{json, Value};

use crate::app::Env;
use crate::cli::{LoginCmd, BIN};
use crate::model::LoginAnswer;
use crate::portal::{self, Api, Device, PortalAddress, PortalInfo};
use crate::session::{Key, Session, KEYCHAIN_SERVICE_ENV};

/// What a profile's config says about its login.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoginSettings {
    /// The portal's web address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<String>,
    /// The clinic's name, from the portal's config.json.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clinic: Option<String>,
    /// The household's label for the patient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl LoginSettings {
    /// The vault entry of the profile's session, once it has signed in.
    pub fn key(&self, profile: &ProfileName) -> Option<Key> {
        let portal = PortalAddress::parse(self.portal.as_deref()?).ok()?;
        Some(Key {
            profile: profile.clone(),
            org: portal.slug,
            patient: self.label.clone().unwrap_or_else(|| profile.to_string()),
        })
    }
}

/// A label names a person in this household (`adult1`), never read from a
/// record: short, printable, no spaces.
pub fn check_label(label: &str) -> Result<(), CliError> {
    let ok =
        !label.is_empty() && label.len() <= 40 && label.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if ok {
        Ok(())
    } else {
        Err(CliError::Usage(
            "a label is up to 40 letters, digits, `-`, `_` or `.` (`adult1`, `child1`)".into(),
        ))
    }
}

/// Where the user id and password come from.
enum Source<'a> {
    Piped,
    OpItem(&'a str),
    Browser,
}

/// The sign-in's outcome, for the caller to store with the profile.
pub struct LoggedIn {
    pub settings: LoginSettings,
}

pub fn login(
    cmd: &LoginCmd,
    profile: &ProfileName,
    stored: &LoginSettings,
    env: &Env,
    json: bool,
    quiet: bool,
) -> Result<LoggedIn, CliError> {
    // Flags first: a refused combination reaches neither the keychain, nor
    // 1Password, nor the portal.
    let piped = cmd.source.stdin || cmd.source.from_env.is_some();
    let source = match (piped, cmd.op_item.as_deref(), cmd.browser.browser) {
        (true, None, false) => Source::Piped,
        (false, Some(item), false) => Source::OpItem(item),
        (false, None, true) => Source::Browser,
        (false, None, false) => {
            return Err(CliError::Usage(format!(
                "pass where the sign-in comes from: --stdin (the user id, then the password, one per line), \
                 --from-env <VAR> (the same two lines), --op-item op://VAULT/ITEM (1Password), or --browser \
                 (the portal's own page); `{BIN} auth login --help`"
            )))
        }
        _ => {
            return Err(CliError::Usage(
                "pass one of --stdin, --from-env, --op-item and --browser".into(),
            ))
        }
    };
    if cmd.source.stdin && cmd.source.from_env.is_some() {
        return Err(CliError::Usage("pass one of --stdin and --from-env".into()));
    }
    let op_refs = match source {
        Source::OpItem(item) => Some(op_refs(item)?),
        _ => None,
    };
    if let Some(label) = &cmd.label {
        check_label(label)?;
    }
    let address = match (cmd.portal.as_deref(), stored.portal.as_deref()) {
        (Some(given), Some(kept)) => {
            let given = PortalAddress::parse(given)?;
            let kept = PortalAddress::parse(kept)?;
            if given != kept {
                return Err(CliError::Usage(format!(
                    "profile `{profile}` signs in to {}; a login at another portal is another profile \
                     (`{BIN} profiles add <NAME>`)",
                    kept.origin
                )));
            }
            given
        }
        (Some(given), None) => PortalAddress::parse(given)?,
        (None, Some(kept)) => PortalAddress::parse(kept)?,
        (None, None) => {
            return Err(CliError::Usage(format!(
                "profile `{profile}` has not signed in yet: pass --portal <PORTAL> (its host, \
                 portal-<clinic>.fertemr.com, or <clinic>)"
            )))
        }
    };
    let label = cmd
        .label
        .clone()
        .or_else(|| stored.label.clone())
        .unwrap_or_else(|| profile.to_string());
    check_label(&label)?;
    let key = Key {
        profile: profile.clone(),
        org: address.slug.clone(),
        patient: label.clone(),
    };
    if stored.label.as_deref().is_some_and(|l| l != label) {
        return Err(CliError::Usage(format!(
            "profile `{profile}` keeps its sign-in as `{}`; `{BIN} auth logout --profile {profile}` first to \
             change the label",
            stored.label.as_deref().unwrap_or_default()
        )));
    }

    let store = (env.sessions)()?;
    // Before anything is read or sent, so no sign-in is spent on a session
    // that would not be stored.
    let previous = match store.get(&key) {
        Ok(found) => found,
        Err(e) if e.reason() == Some(Reason::CredentialUnreadable) => None,
        Err(e) => return Err(e),
    };
    if previous.is_some() && !cmd.overwrite {
        return Err(CliError::Usage(format!(
            "a portal session is already stored for profile `{profile}`; pass --overwrite to sign in again \
             (`{BIN} auth status --check --profile {profile}` says whether the portal still accepts it)"
        )));
    }
    // Piped lines are read and checked before the portal is asked; a
    // 1Password read, which may raise an approval, waits until the portal
    // has answered.
    let piped_pair = match source {
        Source::Piped => Some(two_lines(&cmd.source.read(None)?)?),
        _ => None,
    };
    let info = portal::discover(&address)?;
    let api = Api::new(&info.api_base)?;
    // The device id stays the profile's across sign-ins, as the portal's
    // app keeps its own.
    let device_id = previous
        .as_ref()
        .map(|p| p.device_id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let (device, answer) = match source {
        Source::Piped => {
            let (user, password) = piped_pair.expect("read above");
            let answer = api.sign_in(&device_id, &user, &password)?;
            (session_device(&device_id, &answer)?, answer)
        }
        Source::OpItem(_) => {
            let (user_ref, password_ref) = op_refs.expect("parsed above");
            let op = OnePassword::new();
            let user = op.read(&user_ref)?;
            let password = op.read(&password_ref)?;
            let answer = api.sign_in(&device_id, &user, &password)?;
            (session_device(&device_id, &answer)?, answer)
        }
        Source::Browser => {
            let device = browser_device(profile, &info, cmd.browser.fresh_profile, env.hermetic, quiet)?;
            let answer = api.current_user(&device)?;
            (device, answer)
        }
    };
    let session = Session {
        portal: info.address.origin.clone(),
        api_base: info.api_base.clone(),
        device_id: device.duuid,
        session_id: device.suuid,
        user_id: answer.pp_user_id,
        patient_id: answer.pt_id,
        full_name: answer.full_name.clone(),
        first_name: answer.first_name.clone(),
        last_name: answer.last_name.clone(),
        signed_in_at: crate::app::now_rfc3339(),
    };
    store.put(&key, &session)?;
    if !quiet {
        eprintln!(
            "portal session stored for profile `{profile}` ({})",
            info.address.origin
        );
        if answer.accepted_latest_tos == Some(false) {
            eprintln!(
                "note: the portal asks this account to accept its latest terms in its own app; fertemr reads \
                 without accepting anything"
            );
        }
    }
    if json {
        output::json(&AuthLogin::stored(true).to_json());
    }
    Ok(LoggedIn {
        settings: LoginSettings {
            portal: Some(info.address.origin),
            clinic: Some(info.name),
            label: Some(label),
        },
    })
}

/// The two 1Password references an item names: its `username` and
/// `password` fields. The input is not echoed in an error: it may be a
/// secret pasted where its reference belongs.
fn op_refs(item: &str) -> Result<(OpRef, OpRef), CliError> {
    let item = item.trim_end_matches('/');
    let segments = item.strip_prefix("op://").map(|r| r.split('/').count());
    if segments != Some(2) {
        return Err(CliError::Usage(
            "--op-item is a 1Password item, op://VAULT/ITEM (the value given is not shown)".into(),
        ));
    }
    let field = |name: &str| {
        format!("{item}/{name}").parse::<OpRef>().map_err(|_| {
            CliError::Usage("--op-item is a 1Password item, op://VAULT/ITEM (the value given is not shown)".into())
        })
    };
    Ok((field("username")?, field("password")?))
}

/// The user id and the password, one per line. The error never quotes
/// either.
fn two_lines(raw: &Secret) -> Result<(Secret, Secret), CliError> {
    let text = raw.expose();
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let user = lines.next().unwrap_or_default().trim();
    let password = lines.next().unwrap_or_default();
    let rest_blank = lines.all(|l| l.trim().is_empty());
    if user.is_empty() || password.is_empty() || !rest_blank {
        return Err(CliError::Usage(
            "the sign-in reads two lines: the portal user id, then the password (what was read is not shown)".into(),
        ));
    }
    Ok((Secret::new(user), Secret::new(password)))
}

/// The session a sign-in answered, with the device it was made for.
fn session_device(device_id: &str, answer: &LoginAnswer) -> Result<Device, CliError> {
    let suuid = answer
        .session_id
        .clone()
        .ok_or_else(|| CliError::Upstream("the sign-in succeeded but answered no session".into()))?;
    Ok(Device {
        duuid: device_id.to_string(),
        suuid: Secret::new(suuid),
    })
}

/// The portal's web app keeps its device id and session in local storage,
/// as Flutter does: each value a JSON string under `flutter.<name>`.
const STORAGE_EXPRESSION: &str = "(() => { const d = localStorage.getItem('flutter.appUUID'); \
     const s = localStorage.getItem('flutter.sessUUID'); \
     if (!d || !s || s === '\"\"' || s === 'null') return null; \
     return JSON.stringify({ d: d, s: s }); })()";

/// A session from a Chrome window: the owner signs in on the portal's own
/// page, and the capture reads the session once the page has stored one.
fn browser_device(
    profile: &ProfileName,
    info: &PortalInfo,
    fresh: bool,
    hermetic: bool,
    quiet: bool,
) -> Result<Device, CliError> {
    let scratch = fresh || hermetic;
    if scratch && !fresh && !quiet {
        eprintln!(
            "{KEYCHAIN_SERVICE_ENV} is set, so this sign-in uses a fresh browser profile and leaves the kept one alone"
        );
    }
    let host = info
        .address
        .origin
        .split_once("://")
        .map(|(_, h)| h.split(':').next().unwrap_or(h).to_string())
        .unwrap_or_default();
    let rule = HostRule::exact(&host).map_err(CliError::from)?;
    let capture = Capture::new("portal session", rule)
        .expression(STORAGE_EXPRESSION)
        .hint("sign in on the portal's page; the window closes once the portal shows your home page");
    let launch = Launch::new(BIN)
        .url(format!("{}/", info.address.origin))
        .profile_name(profile.as_str())
        .fresh_profile(scratch)
        .login_hint(format!("{BIN} auth login --browser --profile {profile}"))
        .quiet(quiet);
    let captured = pk_cli_browser::capture(launch, &capture, pk_cli_browser::DEFAULT_TIMEOUT).map_err(|e| match e {
        e @ BrowserError::ChromeNotFound(_) => CliError::from(e).with_remedy(format!(
            "without Chrome, sign in with the password: `{BIN} auth login --stdin --profile {profile}`"
        )),
        e => e.into(),
    })?;
    let raw = captured
        .into_value()
        .ok_or_else(|| CliError::Other("the sign-in window handed over no session".into()))?;
    let pair: Value = serde_json::from_str(raw.expose()).map_err(|_| {
        CliError::Upstream("the portal's page keeps its session in a shape fertemr does not read".into())
    })?;
    let unwrap = |v: &Value| -> Option<String> {
        let s = v.as_str()?;
        // Flutter keeps each value JSON-encoded.
        let s = serde_json::from_str::<String>(s).unwrap_or_else(|_| s.to_string());
        (!s.is_empty()).then_some(s)
    };
    match (unwrap(&pair["d"]), unwrap(&pair["s"])) {
        (Some(d), Some(s)) => Ok(Device {
            duuid: d,
            suuid: Secret::new(s),
        }),
        _ => Err(CliError::Upstream(
            "the portal's page keeps its session in a shape fertemr does not read".into(),
        )),
    }
}

// ---------------------------------------------------------------- status

/// `auth status`: local facts, and with `check` one read of the portal.
pub fn status(
    profile: &ProfileName,
    settings: &LoginSettings,
    check: bool,
    env: &Env,
    json: bool,
    quiet: bool,
) -> Result<(), CliError> {
    let (stored, session) = match settings.key(profile) {
        None => (false, None),
        Some(key) => match (env.sessions)()?.get(&key) {
            Ok(session) => (session.is_some(), session),
            Err(e) if e.reason() == Some(Reason::CredentialUnreadable) => {
                eprintln!("warning: {}", e.message());
                (true, None)
            }
            Err(e) => return Err(e),
        },
    };
    let valid = match (&session, check) {
        (Some(s), true) => match Api::new(&s.api_base)?.current_user(&s.device()) {
            Ok(_) => Some(true),
            Err(e) if e.is_auth() => Some(false),
            Err(e) => return Err(e),
        },
        _ => None,
    };
    let mut status = AuthStatus::new(true, session.is_some() && valid != Some(false), AuthMethod::Password);
    status.account = Some(profile.as_str().to_string());
    status.credential_in_keychain = Some(stored);
    status.session_valid = valid;
    if env.hermetic {
        if !quiet {
            eprintln!("{KEYCHAIN_SERVICE_ENV} is set, so the kept browser profile is not reported");
        }
    } else {
        match pk_cli_browser::named_profile_exists(BIN, profile.as_str()) {
            Ok(kept) => status.browser_profile = Some(kept),
            Err(e) => eprintln!("warning: {e}"),
        }
    }
    if json {
        let mut v = status.to_json();
        if let Some(s) = &session {
            v["portal"] = json!(s.portal);
            v["signed_in_at"] = json!(s.signed_in_at);
        }
        output::json(&v);
    } else {
        status.render();
        if let Some(s) = &session {
            println!("Portal:        {}", s.portal);
            println!("Signed in:     {}", s.signed_in_at);
        }
        if valid == Some(false) && !quiet {
            eprintln!("the portal no longer accepts the session: `{BIN} auth login --overwrite --profile {profile}`");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- logout

/// What `auth logout` did (`auth-logout/v1`).
#[derive(Debug, PartialEq, Eq)]
pub struct LoggedOut {
    pub session_removed: bool,
    /// Whether the portal was told; `None` with no session to end.
    pub signed_out_at_portal: Option<bool>,
    /// `None` when the browser profile was left alone ([`KEYCHAIN_SERVICE_ENV`]).
    pub browser_profile_removed: Option<bool>,
}

impl LoggedOut {
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "schema": "auth-logout/v1", "session_removed": self.session_removed });
        if let Some(told) = self.signed_out_at_portal {
            v["signed_out_at_portal"] = told.into();
        }
        if let Some(removed) = self.browser_profile_removed {
            v["browser_profile_removed"] = removed.into();
        }
        v
    }

    pub fn line(&self) -> String {
        let session = match (self.session_removed, self.signed_out_at_portal) {
            (true, Some(true)) => "signed out at the portal and removed the session from the vault",
            (true, _) => "removed the session from the vault (the portal was not reached)",
            (false, _) => "no session was stored",
        };
        let profile = match self.browser_profile_removed {
            Some(true) => "the browser profile was removed".to_string(),
            Some(false) => "no browser profile was kept".to_string(),
            None => format!("{KEYCHAIN_SERVICE_ENV} is set, so the kept browser profile was left alone"),
        };
        format!("logged out: {session}; {profile}")
    }
}

pub fn logout(
    profile: &ProfileName,
    settings: &LoginSettings,
    env: &Env,
    json: bool,
    quiet: bool,
) -> Result<(), CliError> {
    let done = forget_login(profile, settings, true, env)?;
    if json {
        output::json(&done.to_json());
    } else if !quiet {
        eprintln!("{}", done.line());
    }
    Ok(())
}

/// What a login keeps, removed: the session (ended at the portal first when
/// `at_portal`, which a failure to reach it does not stop), then the browser
/// profile, which a hermetic run ([`Env::hermetic`]) leaves alone.
pub fn forget_login(
    profile: &ProfileName,
    settings: &LoginSettings,
    at_portal: bool,
    env: &Env,
) -> Result<LoggedOut, CliError> {
    let mut told = None;
    let mut removed = false;
    if let Some(key) = settings.key(profile) {
        let store = (env.sessions)()?;
        let session = match store.get(&key) {
            Ok(s) => s,
            Err(e) if e.reason() == Some(Reason::CredentialUnreadable) => None,
            Err(e) => return Err(e),
        };
        if let (Some(s), true) = (&session, at_portal) {
            let ended = Api::new(&s.api_base).and_then(|api| api.sign_out(&s.device()));
            if let Err(e) = &ended {
                eprintln!("warning: the portal was not told: {}", e.message());
            }
            told = Some(ended.is_ok());
        }
        removed = store.delete(&key)?;
    }
    let browser = if env.hermetic {
        None
    } else {
        Some(pk_cli_browser::forget_named_profile(BIN, profile.as_str()).map_err(CliError::from)?)
    };
    Ok(LoggedOut {
        session_removed: removed,
        signed_out_at_portal: told,
        browser_profile_removed: browser,
    })
}

// ----------------------------------------------------------------- reset

/// `auth reset`: every profile's session and the vault's key, for a key
/// that is lost or damaged. Asks on a terminal and needs `--force`
/// otherwise (exit 6), decided before the vault is opened.
pub fn reset(force: bool, interactive: bool, env: &Env, json: bool, quiet: bool) -> Result<(), CliError> {
    pk_cli_core::confirm::require_confirmable(
        force,
        interactive,
        "remove every profile's portal session and the vault's key",
    )?;
    pk_cli_core::confirm::confirm(
        force,
        "Remove every profile's stored portal session and the vault's key? Each profile signs in again after",
    )?;
    let removed = (env.sessions)()?.clear()?;
    if json {
        output::json(&json!({ "schema": "auth-reset/v1", "sessions_removed": removed }));
    } else if !quiet {
        eprintln!("reset: {removed} stored session(s) and the vault's key removed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_reads_two_lines_and_never_quotes_them() {
        let (u, p) = two_lines(&Secret::new("someone\nhunter2\n")).unwrap();
        assert_eq!((u.expose(), p.expose()), ("someone", "hunter2"));
        let (u, p) = two_lines(&Secret::new("someone\r\npass word\r\n")).unwrap();
        assert_eq!((u.expose(), p.expose()), ("someone", "pass word"));
        for bad in ["", "someone", "someone\n", "\nhunter2", "a\nb\nc"] {
            let e = two_lines(&Secret::new(bad)).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{bad:?}");
            assert!(
                !e.message().contains("hunter2") && !e.message().contains("someone"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_op_item_names_its_two_fields() {
        let (u, p) = op_refs("op://Private/Some Login").unwrap();
        assert_eq!(u.as_str(), "op://Private/Some Login/username");
        assert_eq!(p.as_str(), "op://Private/Some Login/password");
        for bad in ["op://Private", "op://Private/Item/password", "Private/Item", "hunter2"] {
            let e = op_refs(bad).unwrap_err();
            assert!(!e.message().contains(bad), "{bad}");
        }
    }

    #[test]
    fn a_label_is_short_and_plain() {
        check_label("adult1").unwrap();
        check_label("child-2.b").unwrap();
        for bad in ["", "two words", "a/b", &"x".repeat(41)] {
            assert_eq!(check_label(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }

    #[test]
    fn the_key_needs_a_portal() {
        let p = ProfileName::parse("adult1").unwrap();
        assert_eq!(LoginSettings::default().key(&p), None);
        let s = LoginSettings {
            portal: Some("https://portal-someclinic.fertemr.com".into()),
            clinic: None,
            label: None,
        };
        assert_eq!(s.key(&p).unwrap().to_string(), "adult1/someclinic/adult1");
    }
}
