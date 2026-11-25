//! The test suite's stub portal as a process, for trying fertemr by hand on
//! synthetic data: `cargo run --example stub-portal`, then sign in to the
//! address it prints with the user id and password it prints.
//!
//! Point fertemr at a scratch vault while doing so, so nothing reaches the
//! real one:
//!
//! ```sh
//! export XDG_CONFIG_HOME=$(mktemp -d) FERTEMR_KEYCHAIN_SERVICE=piekstra.fertemr.stub
//! ```
//!
//! `STUB_SCENARIO` takes a comma-separated list of the cases the tests use:
//! `no-forms`, `visits-fail`, `bad-pdf`, `no-calendar`, `logout-fails`,
//! `expire-after=N`.

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let mut scenario = common::Scenario::default();
    for case in std::env::var("STUB_SCENARIO").unwrap_or_default().split(',') {
        match case.trim() {
            "" => {}
            "no-forms" => scenario.forms_offered = false,
            "visits-fail" => scenario.visits_fail = true,
            "bad-pdf" => scenario.bad_pdf = true,
            "no-calendar" => scenario.calendar = false,
            "logout-fails" => scenario.logout_fails = true,
            other => match other.strip_prefix("expire-after=").map(str::parse) {
                Some(Ok(n)) => scenario.expire_after = Some(n),
                _ => {
                    eprintln!("unknown STUB_SCENARIO case `{other}`");
                    std::process::exit(2);
                }
            },
        }
    }
    let stub = common::Stub::start(scenario);
    println!("portal   {}", stub.origin());
    println!("user     {}", common::USER);
    println!("password {}", common::PASSWORD);
    loop {
        std::thread::park();
    }
}
