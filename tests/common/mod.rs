//! A stub FERT portal on 127.0.0.1, answering from `tests/fixtures/portal`.
//!
//! It serves the portal's public config.json, the sign-in, the user read,
//! the sign-out, every list fertemr reads and three downloads, and holds one
//! session: the device id and session id of the last successful sign-in.
//! A read with another pair answers as the real portal does for a session
//! it does not hold. Every request is logged by path, so a test can say what
//! was and was not sent.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

pub const USER: &str = "user@example.com";
pub const PASSWORD: &str = "right-password";
pub const SESSION: &str = "dummy-session-0001";

/// A PDF small enough to read, whole enough to pass `verify_pdf`.
pub fn pdf(label: &str) -> Vec<u8> {
    format!("%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n% {label}\ntrailer << /Root 1 0 R >>\n%%EOF\n").into_bytes()
}

pub fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/portal")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// What the stub answers, beyond the fixtures.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// The intake forms answer "not found", as a list not offered does.
    pub forms_offered: bool,
    /// The visits list answers HTTP 500 with an envelope of its own.
    pub visits_fail: bool,
    /// The session ends after this many reads (sign-in excluded).
    pub expire_after: Option<usize>,
    /// The lab report PDF is served as HTML instead.
    pub bad_pdf: bool,
    /// config.json names this API base instead of the stub's own.
    pub api_base: Option<String>,
    /// config.json answers 404.
    pub no_config: bool,
    /// The treatment calendar holds events.
    pub calendar: bool,
    /// The sign-out fails with an error of the portal's own.
    pub logout_fails: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            forms_offered: true,
            visits_fail: false,
            expire_after: None,
            bad_pdf: false,
            api_base: None,
            no_config: false,
            calendar: true,
            logout_fails: false,
        }
    }
}

#[derive(Default)]
struct State {
    /// (duuid, suuid) of the session the portal holds.
    session: Option<(String, String)>,
    reads: usize,
    log: Vec<String>,
    bodies: Vec<(String, Value)>,
    sign_ins: usize,
}

pub struct Stub {
    pub port: u16,
    state: Arc<Mutex<State>>,
}

impl Stub {
    pub fn start(scenario: Scenario) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let state = shared.clone();
                let scenario = scenario.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, port, &state, &scenario);
                });
            }
        });
        Stub { port, state }
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Every request's path, in order (`GET /config.json`, `POST PPLab/List`).
    pub fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }

    pub fn bodies(&self, path: &str) -> Vec<Value> {
        self.state
            .lock()
            .unwrap()
            .bodies
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }

    pub fn sign_ins(&self) -> usize {
        self.state.lock().unwrap().sign_ins
    }

    pub fn holds_session(&self) -> bool {
        self.state.lock().unwrap().session.is_some()
    }

    /// The portal ends its session, as it does after its own idle limit.
    pub fn end_session(&self) {
        self.state.lock().unwrap().session = None;
    }
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn respond(mut stream: &TcpStream, status: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn serve(stream: TcpStream, port: u16, state: &Mutex<State>, scenario: &Scenario) -> std::io::Result<()> {
    let req = read_request(&stream)?;
    let json = "application/json; charset=utf-8";
    let api = req.path.strip_prefix("/api/Portal/").map(str::to_string);
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    {
        let mut s = state.lock().unwrap();
        let shown = api.clone().unwrap_or_else(|| req.path.clone());
        s.log.push(format!("{} {shown}", req.method));
        s.bodies.push((shown, body.clone()));
    }
    if req.path == "/config.json" {
        if scenario.no_config {
            return respond(&stream, 404, "text/html", b"<html>not here</html>");
        }
        let base = scenario
            .api_base
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}/api"));
        return respond(
            &stream,
            200,
            json,
            fixture("config.json").replace("{BASE}", &base).as_bytes(),
        );
    }
    let Some(path) = api else {
        return respond(&stream, 404, "text/html", b"<html>not here</html>");
    };
    if path == "User/LoginAttempt" {
        let ok = body["username"] == USER && body["password"] == PASSWORD;
        if !ok {
            return respond(&stream, 500, json, fixture("login-refused.json").as_bytes());
        }
        let device = req.headers.get("duuid").cloned().unwrap_or_default();
        let mut s = state.lock().unwrap();
        s.session = Some((device, SESSION.into()));
        s.sign_ins += 1;
        s.reads = 0;
        return respond(&stream, 200, json, fixture("login-ok.json").as_bytes());
    }
    // Every other call needs the session the portal holds.
    let held = {
        let mut s = state.lock().unwrap();
        let pair = (
            req.headers.get("duuid").cloned().unwrap_or_default(),
            req.headers.get("suuid").cloned().unwrap_or_default(),
        );
        let mut held = s.session.as_ref() == Some(&pair);
        if held && path != "User/Logout" {
            s.reads += 1;
            if scenario.expire_after.is_some_and(|n| s.reads > n) {
                s.session = None;
                held = false;
            }
        }
        held
    };
    if path == "User/Logout" && scenario.logout_fails {
        return respond(
            &stream,
            500,
            json,
            br#"{"c":9,"t":"Something went wrong","d":null,"r":null}"#,
        );
    }
    if !held {
        return respond(&stream, 500, json, fixture("session-gone.json").as_bytes());
    }
    let answer = |name: &str| respond(&stream, 200, json, fixture(name).as_bytes());
    match path.as_str() {
        "User/GetUserPrefs" => answer("user.json"),
        "User/Logout" => {
            state.lock().unwrap().session = None;
            respond(&stream, 200, json, br#"{"success":true,"msg":"","id":0}"#)
        }
        "PPVisits/List" if scenario.visits_fail => respond(
            &stream,
            500,
            json,
            br#"{"c":9,"t":"Something went wrong","d":null,"r":null}"#,
        ),
        "PPVisits/List" => answer("visits.json"),
        "PPLab/List" => answer("labs.json"),
        "PPEmbryology/List" => answer("embryology.json"),
        "PPFiles/List" => answer("files.json"),
        "PPMessaging/List" if body["Folder"] == "Inbox" => answer("messages-inbox.json"),
        "PPMessaging/List" => answer("messages-empty.json"),
        "PPHP/ListInstances" if scenario.forms_offered => answer("hp-instances.json"),
        "PPHP/ListInstances" => respond(&stream, 500, json, fixture("not-found.json").as_bytes()),
        "PPHP/Instance" if body["id"] == 1301 => answer("hp-instance.json"),
        "PPCalendar/List" if scenario.calendar => answer("calendar.json"),
        "PPCalendar/List" => answer("calendar-empty.json"),
        "PPConfigSettings/List" => respond(&stream, 200, json, br#"{"c":0,"t":"","d":{"Results":[]},"r":null}"#),
        "PPFiles/DownloadFile" if body["FileID"] == 1001 => respond(&stream, 200, "application/pdf", &pdf("consent")),
        "PPLab/DownloadFile" if body["FileID"] == 801 && body["RecordTypeID"] == 1 => {
            if scenario.bad_pdf {
                respond(&stream, 200, "application/pdf", b"<html>an error page</html>")
            } else {
                respond(&stream, 200, "application/pdf", &pdf("lab report"))
            }
        }
        "PPMessaging/DownloadAttachment" if body["FileID"] == 1201 => {
            respond(&stream, 200, "application/pdf", &pdf("letter"))
        }
        "PPMessaging/DownloadAttachment" if body["FileID"] == 1202 => {
            respond(&stream, 200, "application/pdf", &pdf("summary"))
        }
        _ => respond(&stream, 500, json, fixture("not-found.json").as_bytes()),
    }
}
