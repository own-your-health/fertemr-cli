//! fertemr end to end against a stub portal (`common::Stub`), in process,
//! with the sessions in an in-memory vault (`Vault::new(MemoryKeys, dir)`):
//! no keychain item is read and no real portal is reached.
//!
//! The sign-in reads its two lines from an environment variable
//! (`--from-env`), set while the tests hold one lock, so they run one at a
//! time.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use clap::{CommandFactory, FromArgMatches};
use common::{Scenario, Stub, PASSWORD, USER};
use fertemr::app::{self, Config, Env, Given};
use fertemr::auth::LoginSettings;
use fertemr::cli::Cli;
use fertemr::session::{Key, SessionStore};
use pk_cli_auth::vault::{MemoryKeys, Vault};
use pk_cli_config::profile::ProfileName;
use pk_cli_config::ConfigStore;
use serde_json::Value;

const SIGN_IN_VAR: &str = "FERTEMR_E2E_SIGN_IN";
const WRONG_VAR: &str = "FERTEMR_E2E_WRONG_SIGN_IN";

static LOCK: Mutex<()> = Mutex::new(());
static N: AtomicUsize = AtomicUsize::new(0);

/// The calls a pull may send; anything else would be a write.
const READS: &[&str] = &[
    "GET /config.json",
    "POST User/LoginAttempt",
    "GET User/GetUserPrefs",
    "GET User/Logout",
    "POST PPVisits/List",
    "POST PPLab/List",
    "POST PPEmbryology/List",
    "POST PPFiles/List",
    "POST PPMessaging/List",
    "POST PPHP/ListInstances",
    "POST PPHP/Instance",
    "POST PPCalendar/List",
    "POST PPFiles/DownloadFile",
    "POST PPLab/DownloadFile",
    "POST PPMessaging/DownloadAttachment",
];

struct H {
    dir: PathBuf,
    keys: Arc<MemoryKeys>,
    stub: Stub,
    _lock: MutexGuard<'static, ()>,
}

impl H {
    fn new(scenario: Scenario) -> H {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the tests in this file hold LOCK, so no other thread of
        // theirs reads the environment meanwhile.
        unsafe {
            std::env::set_var(SIGN_IN_VAR, format!("{USER}\n{PASSWORD}"));
            std::env::set_var(WRONG_VAR, format!("{USER}\nwrong-password"));
            std::env::remove_var("FERTEMR_PROFILE");
        }
        let dir = std::env::temp_dir().join(format!(
            "fertemr-e2e-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        H {
            dir,
            keys: Arc::new(MemoryKeys::new()),
            stub: Stub::start(scenario),
            _lock: lock,
        }
    }

    fn config(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    /// One command's exit code.
    fn run(&self, args: &[&str]) -> i32 {
        let config = self.config().display().to_string();
        let mut full = vec!["fertemr", "-q", "--config", &config];
        full.extend_from_slice(args);
        let m = Cli::command()
            .try_get_matches_from(&full)
            .unwrap_or_else(|e| panic!("{full:?}: {e}"));
        let cli = Cli::from_arg_matches(&m).unwrap();
        let vault_dir = self.dir.join("vault");
        let keys = self.keys.clone();
        let open = move || Ok(SessionStore::with_vault(Vault::new(keys.clone(), &vault_dir)));
        let env = Env {
            sessions: &open,
            hermetic: true,
        };
        match app::run(&cli, Given::of(&m), &env) {
            Ok(code) => code,
            Err(e) => e.exit_code(),
        }
    }

    fn store(&self) -> SessionStore {
        SessionStore::with_vault(Vault::new(self.keys.clone(), self.dir.join("vault")))
    }

    fn key(&self, profile: &str) -> Key {
        Key {
            profile: ProfileName::parse(profile).unwrap(),
            org: format!("127.0.0.1-{}", self.stub.port),
            patient: profile.into(),
        }
    }

    fn settings(&self, profile: &str) -> LoginSettings {
        let cfg: Config = ConfigStore::new("fertemr")
            .with_override(Some(self.config()))
            .load()
            .unwrap();
        cfg.profiles.settings(&ProfileName::parse(profile).unwrap()).unwrap()
    }

    fn signed_in(&self, profile: &str) {
        assert_eq!(self.run(&["profiles", "add", profile]), 0);
        let portal = self.stub.origin();
        assert_eq!(
            self.run(&[
                "auth",
                "login",
                "--profile",
                profile,
                "--portal",
                &portal,
                "--from-env",
                SIGN_IN_VAR
            ]),
            0
        );
    }

    fn writes(&self) -> Vec<String> {
        self.stub
            .log()
            .into_iter()
            .filter(|l| !READS.contains(&l.as_str()))
            .collect()
    }
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The stream's lines: header, resources, trailer.
fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn ids(stream: &[Value], kind: &str) -> Vec<String> {
    stream
        .iter()
        .filter(|r| r["resourceType"] == kind)
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(stream: &'a [Value], kind: &str, id: &str) -> &'a Value {
    stream
        .iter()
        .find(|r| r["resourceType"] == kind && r["id"] == id)
        .unwrap_or_else(|| panic!("no {kind}/{id}"))
}

#[test]
fn sign_in_export_documents_and_sign_out() {
    let h = H::new(Scenario::default());
    h.signed_in("adult1");
    assert_eq!(h.stub.sign_ins(), 1);
    let session = h.store().get(&h.key("adult1")).unwrap().expect("a session is stored");
    assert_eq!(session.patient_id, Some(303));
    assert_eq!(session.full_name.as_deref(), Some("Jane Example"));
    let settings = h.settings("adult1");
    assert_eq!(settings.portal.as_deref(), Some(h.stub.origin().as_str()));
    assert_eq!(settings.clinic.as_deref(), Some("Example Fertility Clinic"));
    assert_eq!(settings.label.as_deref(), Some("adult1"));
    // The device id the sign-in sent is the one the session is bound to.
    assert!(!session.device_id.is_empty());

    assert_eq!(h.run(&["auth", "status", "--check", "--profile", "adult1"]), 0);
    assert_eq!(h.run(&["patients", "list", "--profile", "adult1"]), 0);

    let out = h.dir.join("export");
    let out_s = out.display().to_string();
    assert_eq!(h.run(&["records", "export", "--profile", "adult1", "-o", &out_s]), 0);
    let text = std::fs::read_to_string(out.join("records.ndjson")).unwrap();
    // A value keeps the portal's spelling.
    assert!(text.contains(r#""value":1.50"#), "1.50 was respelled");
    let stream = lines(&out.join("records.ndjson"));
    let header = &stream[0];
    assert_eq!(header["schema"], "records-stream/v1");
    assert_eq!(header["method"], "portal");
    assert_eq!(header["patient"]["id"], "303");
    assert_eq!(header["patient"]["label"], "adult1");
    assert_eq!(header["organization"]["vendor"], "fert");
    let trailer = stream.last().unwrap();
    assert_eq!(trailer["complete"], true, "{trailer}");
    assert_eq!(trailer["skipped"].as_array().unwrap().len(), 0, "{trailer}");

    assert_eq!(ids(&stream, "Patient"), ["303"]);
    assert_eq!(ids(&stream, "Encounter"), ["visit-501", "visit-502"]);
    assert_eq!(find(&stream, "Encounter", "visit-501")["status"], "finished");
    assert_eq!(find(&stream, "Encounter", "visit-502")["status"], "planned");
    assert_eq!(
        ids(&stream, "Observation"),
        ["lab-7001", "lab-7002", "lab-7003", "embryo-901"]
    );
    assert_eq!(ids(&stream, "DiagnosticReport"), ["lab-order-601", "lab-order-602"]);
    assert_eq!(
        find(&stream, "DiagnosticReport", "lab-order-601")["presentedForm"][0]["url"],
        "Binary/labpdf-801-1"
    );
    assert_eq!(
        find(&stream, "Observation", "lab-7002")["valueQuantity"]["comparator"],
        "<"
    );
    assert_eq!(find(&stream, "Observation", "lab-7003")["valueString"], "Negative");
    assert_eq!(find(&stream, "Observation", "lab-7003")["status"], "preliminary");
    // Drafts stay out; a reply names the message it answers.
    assert_eq!(ids(&stream, "Communication"), ["msg-1101", "msg-1102", "msg-1103"]);
    assert_eq!(
        find(&stream, "Communication", "msg-1102")["inResponseTo"][0]["reference"],
        "Communication/msg-1101"
    );
    assert_eq!(
        ids(&stream, "DocumentReference"),
        ["file-1001", "file-1002", "att-1201", "calendar"]
    );
    assert_eq!(
        find(&stream, "DocumentReference", "file-1002")["content"][0]["attachment"]["url"],
        "https://example.com/education"
    );
    assert_eq!(
        ids(&stream, "Binary"),
        ["labpdf-801-1", "file-1001", "att-1201", "calendar"]
    );
    // Each Binary right after the resource that names it.
    for (i, r) in stream.iter().enumerate() {
        if r["resourceType"] == "Binary" {
            let before = &stream[i - 1];
            let named = before
                .to_string()
                .contains(&format!("Binary/{}", r["id"].as_str().unwrap()));
            assert!(named, "Binary/{} follows {}", r["id"], before["resourceType"]);
        }
    }
    let form = find(&stream, "QuestionnaireResponse", "form-1301");
    assert_eq!(form["status"], "completed");
    let sections = form["item"][0]["item"].as_array().unwrap();
    assert_eq!(sections.len(), 2, "the unanswered section is left out");
    assert_eq!(sections[1]["item"][0]["answer"][0]["valueString"], "No");
    assert_eq!(sections[1]["item"][1]["answer"].as_array().unwrap().len(), 2);
    for r in &stream[1..stream.len() - 1] {
        assert_eq!(r["meta"]["source"], "fertemr:portal", "{}", r["id"]);
    }
    // Inbox, Sent and Archive, each asked for as the app asks.
    let folders: Vec<Value> = h
        .stub
        .bodies("PPMessaging/List")
        .iter()
        .map(|b| b["Folder"].clone())
        .collect();
    assert_eq!(folders, ["Inbox", "Sent", "Archive"]);
    assert_eq!(h.stub.bodies("PPMessaging/List")[0]["PageSize"], 1000);

    // Documents: list, one, all.
    assert_eq!(h.run(&["documents", "list", "--profile", "adult1"]), 0);
    let one = h.dir.join("one.pdf").display().to_string();
    assert_eq!(
        h.run(&["documents", "download", "att-1201", "--profile", "adult1", "-o", &one]),
        0
    );
    assert!(std::fs::read(&one).unwrap().starts_with(b"%PDF"));
    assert_eq!(
        h.run(&["documents", "download", "att-1201", "--profile", "adult1", "-o", &one]),
        2,
        "no overwrite"
    );
    let all = h.dir.join("all");
    let all_s = all.display().to_string();
    assert_eq!(
        h.run(&["documents", "download", "--all", "--profile", "adult1", "-o", &all_s]),
        0
    );
    let mut saved: Vec<String> = std::fs::read_dir(&all)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    saved.sort();
    assert_eq!(
        saved,
        ["Example consent.pdf", "Example instructions.pdf", "Example report.pdf"]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(all.join("Example consent.pdf"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "a saved document is owner-only");
    }
    // Again into the same folder: each file is already there, so nothing is
    // saved twice.
    assert_eq!(
        h.run(&["documents", "download", "--all", "--profile", "adult1", "-o", &all_s]),
        0
    );
    assert_eq!(
        std::fs::read_dir(&all).unwrap().count(),
        3,
        "a repeat saves no second copy"
    );
    // A different file holding a document's name leaves it saved beside it.
    std::fs::write(all.join("Example consent.pdf"), b"%PDF-1.4 another file").unwrap();
    assert_eq!(
        h.run(&["documents", "download", "--all", "--profile", "adult1", "-o", &all_s]),
        0
    );
    assert!(all.join("file-1001_Example consent.pdf").exists());
    assert_eq!(std::fs::read_dir(&all).unwrap().count(), 4);
    assert_eq!(
        h.run(&["documents", "download", "file-9", "--profile", "adult1", "-o", &one]),
        4
    );
    assert_eq!(
        h.run(&["documents", "list", "--profile", "adult1", "--since", "2025-02-30"]),
        2,
        "a day the month does not have"
    );

    // The passthrough reads, and refuses what is not a read before sending.
    assert_eq!(h.run(&["api", "PPLab/List", "--profile", "adult1"]), 0);
    let before = h.stub.log().len();
    assert_eq!(h.run(&["api", "PPMessaging/SetReadStatus", "--profile", "adult1"]), 2);
    assert_eq!(h.run(&["api", "PPFiles/DownloadFile", "--profile", "adult1"]), 2);
    assert_eq!(
        h.run(&["api", "PPLab/List", "--data", "not json", "--profile", "adult1"]),
        2
    );
    assert_eq!(h.stub.log().len(), before, "a refused passthrough sends nothing");
    assert_eq!(
        h.writes(),
        Vec::<String>::new(),
        "fertemr sent something other than a read"
    );

    // A second sign-in needs --overwrite and sends nothing without it.
    let portal = h.stub.origin();
    assert_eq!(
        h.run(&["auth", "login", "--profile", "adult1", "--from-env", SIGN_IN_VAR]),
        2
    );
    assert_eq!(h.stub.sign_ins(), 1);
    assert_eq!(
        h.run(&[
            "auth",
            "login",
            "--profile",
            "adult1",
            "--from-env",
            SIGN_IN_VAR,
            "--overwrite"
        ]),
        0
    );
    assert_eq!(h.stub.sign_ins(), 2);
    let again = h.store().get(&h.key("adult1")).unwrap().unwrap();
    assert_eq!(again.device_id, session.device_id, "the device id stays the profile's");
    // Another portal is another profile.
    assert_eq!(
        h.run(&[
            "auth",
            "login",
            "--profile",
            "adult1",
            "--portal",
            "someclinic",
            "--from-env",
            SIGN_IN_VAR,
            "--overwrite"
        ]),
        2
    );
    let _ = portal;

    // Sign-out ends the session at the portal and in the vault.
    assert_eq!(h.run(&["auth", "logout", "--profile", "adult1"]), 0);
    assert!(!h.stub.holds_session());
    assert!(h.store().get(&h.key("adult1")).unwrap().is_none());
    assert_eq!(h.run(&["records", "export", "--profile", "adult1", "-o", &out_s]), 3);
    assert_eq!(h.run(&["auth", "status", "--profile", "adult1"]), 0);
}

#[test]
fn since_leaves_out_what_is_dated_before() {
    let h = H::new(Scenario::default());
    h.signed_in("adult1");
    let out = h.dir.join("since");
    let out_s = out.display().to_string();
    assert_eq!(
        h.run(&[
            "records",
            "export",
            "--profile",
            "adult1",
            "--since",
            "2025-01-01",
            "-o",
            &out_s
        ]),
        0
    );
    let stream = lines(&out.join("records.ndjson"));
    assert_eq!(stream[0]["since"], "2025-01-01");
    assert!(stream[0]["since_unbounded"]
        .as_array()
        .unwrap()
        .contains(&"Observation".into()));
    assert_eq!(ids(&stream, "DiagnosticReport"), ["lab-order-601"]);
    assert_eq!(ids(&stream, "Communication"), ["msg-1101", "msg-1102"]);
    // Undated embryology stays in.
    assert!(ids(&stream, "Observation").contains(&"embryo-901".to_string()));
    assert_eq!(
        h.run(&[
            "records",
            "export",
            "--profile",
            "adult1",
            "--since",
            "2025-13-01",
            "-o",
            &out_s
        ]),
        2
    );
}

#[test]
fn a_session_the_portal_ends_mid_export_ends_the_stream_incomplete() {
    let h = H::new(Scenario {
        expire_after: Some(3),
        ..Scenario::default()
    });
    h.signed_in("adult1");
    let out = h.dir.join("cut");
    let out_s = out.display().to_string();
    assert_eq!(h.run(&["records", "export", "--profile", "adult1", "-o", &out_s]), 3);
    assert!(!out.join("records.ndjson").exists());
    let stream = lines(&out.join("records.incomplete.ndjson"));
    let trailer = stream.last().unwrap();
    assert_eq!(trailer["complete"], false);
    assert_eq!(trailer["error"]["code"], "auth");
    assert_eq!(trailer["error"]["exit_code"], 3);
}

#[test]
fn what_the_portal_does_not_serve_is_a_skip() {
    let h = H::new(Scenario {
        embryology_offered: false,
        files_fail: true,
        bad_pdf: true,
        calendar: false,
        ..Scenario::default()
    });
    h.signed_in("adult1");
    let out = h.dir.join("skips");
    let out_s = out.display().to_string();
    assert_eq!(h.run(&["records", "export", "--profile", "adult1", "-o", &out_s]), 0);
    let stream = lines(&out.join("records.ndjson"));
    let trailer = stream.last().unwrap();
    assert_eq!(trailer["complete"], true);
    let skips: Vec<(String, String)> = trailer["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["code"].as_str().unwrap().to_string(),
                s["search"]
                    .as_str()
                    .or(s["id"].as_str())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert!(
        skips.contains(&("unsupported".into(), "PPEmbryology/List".into())),
        "{skips:?}"
    );
    assert!(skips.contains(&("upstream".into(), "PPFiles/List".into())), "{skips:?}");
    assert!(
        skips.contains(&("verify_failed".into(), "lab-order-601".into())),
        "{skips:?}"
    );
    // The report stays, without the file that did not verify.
    let report = find(&stream, "DiagnosticReport", "lab-order-601");
    assert!(report.get("presentedForm").is_none());
    assert!(!ids(&stream, "Binary").contains(&"labpdf-801-1".to_string()));
    assert!(ids(&stream, "Observation").iter().all(|id| !id.starts_with("embryo-")));
    assert!(!ids(&stream, "DocumentReference").contains(&"calendar".to_string()));
    // documents list says what it could not read and lists the rest.
    assert_eq!(h.run(&["documents", "list", "--profile", "adult1"]), 0);
}

#[test]
fn a_refused_sign_in_stores_nothing_and_is_sent_once() {
    let h = H::new(Scenario::default());
    assert_eq!(h.run(&["profiles", "add", "adult2"]), 0);
    let portal = h.stub.origin();
    assert_eq!(
        h.run(&[
            "auth",
            "login",
            "--profile",
            "adult2",
            "--portal",
            &portal,
            "--from-env",
            WRONG_VAR
        ]),
        3
    );
    let attempts = h.stub.log().iter().filter(|l| l.ends_with("User/LoginAttempt")).count();
    assert_eq!(attempts, 1, "a refused sign-in is not retried");
    assert!(h.store().get(&h.key("adult2")).unwrap().is_none());
    assert_eq!(
        h.settings("adult2").portal,
        None,
        "nothing is kept of a refused sign-in"
    );
    // No session: every reading command is exit 3, before any request.
    let before = h.stub.log().len();
    for cmd in [
        &["patients", "list"][..],
        &["records", "export", "--ndjson"],
        &["documents", "list"],
        &["api", "PPLab/List"],
    ] {
        let mut args = cmd.to_vec();
        args.extend(["--profile", "adult2"]);
        assert_eq!(h.run(&args), 3, "{cmd:?}");
    }
    assert_eq!(h.stub.log().len(), before);
}

#[test]
fn a_portal_that_is_not_one_gets_no_sign_in() {
    let h = H::new(Scenario {
        no_config: true,
        ..Scenario::default()
    });
    assert_eq!(h.run(&["profiles", "add", "adult1"]), 0);
    let portal = h.stub.origin();
    let login = [
        "auth",
        "login",
        "--profile",
        "adult1",
        "--portal",
        &portal,
        "--from-env",
        SIGN_IN_VAR,
    ];
    assert_eq!(h.run(&login), 4);
    assert_eq!(h.run(&["orgs", "show", &portal]), 4);
    // The lock is one harness's at a time.
    drop(h);

    let h = H::new(Scenario {
        api_base: Some("https://api.example.com".into()),
        ..Scenario::default()
    });
    assert_eq!(h.run(&["profiles", "add", "adult1"]), 0);
    let portal = h.stub.origin();
    let login = [
        "auth",
        "login",
        "--profile",
        "adult1",
        "--portal",
        &portal,
        "--from-env",
        SIGN_IN_VAR,
    ];
    assert_eq!(h.run(&login), 5);
    assert_eq!(h.stub.sign_ins(), 0);
    assert!(!h.stub.log().iter().any(|l| l.ends_with("LoginAttempt")));
}

#[test]
fn a_session_the_portal_dropped_reads_as_signed_out() {
    let h = H::new(Scenario::default());
    h.signed_in("adult1");
    h.stub.end_session();
    // status --check answers, exit 0, and says the session is not valid.
    assert_eq!(h.run(&["auth", "status", "--check", "--profile", "adult1"]), 0);
    assert_eq!(h.run(&["documents", "list", "--profile", "adult1"]), 3);
    let out = h.dir.join("gone").display().to_string();
    assert_eq!(h.run(&["records", "export", "--profile", "adult1", "-o", &out]), 3);
    // Signing out a session the portal already ended: the portal says so,
    // which is signed out, and the session goes.
    let session = h.store().get(&h.key("adult1")).unwrap().unwrap();
    let api = fertemr::portal::Api::new(&session.api_base).unwrap();
    assert!(api.sign_out(&session.device()).is_ok());
    assert_eq!(h.run(&["auth", "logout", "--profile", "adult1"]), 0);
    assert!(h.store().get(&h.key("adult1")).unwrap().is_none());
}

#[test]
fn a_sign_out_the_portal_fails_still_removes_the_session() {
    let h = H::new(Scenario {
        logout_fails: true,
        ..Scenario::default()
    });
    h.signed_in("adult1");
    let session = h.store().get(&h.key("adult1")).unwrap().unwrap();
    let api = fertemr::portal::Api::new(&session.api_base).unwrap();
    let e = api.sign_out(&session.device()).unwrap_err();
    assert_eq!(e.exit_code(), 5, "an error of the portal's is not a sign-out");
    assert_eq!(h.run(&["auth", "logout", "--profile", "adult1"]), 0);
    assert!(h.store().get(&h.key("adult1")).unwrap().is_none());
    assert!(h.stub.holds_session(), "the portal was not told");
}

#[test]
fn profiles_and_reset() {
    let h = H::new(Scenario::default());
    h.signed_in("adult1");
    assert_eq!(h.run(&["profiles", "add", "adult2"]), 0);
    assert_eq!(h.run(&["profiles", "list"]), 0);
    assert_eq!(h.run(&["orgs", "list"]), 0);
    // No profile, or one not configured: exit 2.
    assert_eq!(h.run(&["patients", "list"]), 2);
    assert_eq!(h.run(&["patients", "list", "--profile", "adult9"]), 2);
    // A command that needs no sign-in takes no --profile.
    assert_eq!(h.run(&["profiles", "list", "--profile", "adult1"]), 2);
    assert_eq!(h.run(&["profiles", "remove", "adult1", "--force"]), 0);
    assert!(h.store().get(&h.key("adult1")).unwrap().is_none());
    assert_eq!(h.run(&["profiles", "remove", "adult1", "--force"]), 4);
    h.signed_in("adult3");
    assert_eq!(h.run(&["auth", "reset", "--force"]), 0);
    assert!(h.store().get(&h.key("adult3")).unwrap().is_none());
}

/// The current user comes in the portal's envelope: a live session reads as
/// valid and names the patient, and one the portal ended reads as signed out.
#[test]
fn the_current_user_read_tells_a_live_session_from_an_ended_one() {
    let h = H::new(Scenario::default());
    h.signed_in("adult1");
    let session = h.store().get(&h.key("adult1")).unwrap().unwrap();
    let api = fertemr::portal::Api::new(&session.api_base).unwrap();
    let user = api.current_user(&session.device()).unwrap();
    assert_eq!(user.pt_id, Some(303));
    h.stub.end_session();
    assert!(api.current_user(&session.device()).unwrap_err().is_auth());
}
