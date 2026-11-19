//! The command tree (piekstra-cli/1, health-records/v1, documents/v1).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_browser::BrowserArgs;
use pk_cli_core::{CommonArgs, RangeArgs};
use pk_cli_secrets::SecretSourceArgs;
use pk_cli_selfupdate::SelfUpdateArgs;

pub const BIN: &str = "fertemr";

/// The environment's fallback for --profile.
pub const PROFILE_ENV: &str = "FERTEMR_PROFILE";

/// Patient portals on the FERT fertility EMR (portal-<clinic>.fertemr.com):
/// lab results, visits, embryology, documents, messages and intake forms,
/// as health-records/v1 for a records store. Read-only; unofficial, not
/// affiliated with or endorsed by Redshift Technologies or any clinic.
#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Override the config file location.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// The login to use: a profile added with `fertemr profiles add <NAME>`
    /// and signed in with `fertemr auth login --profile <NAME>`. There is no
    /// default profile.
    #[arg(long, global = true, env = PROFILE_ENV, value_name = "NAME")]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The portal sign-in, one per --profile.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// The logins: one profile per portal account (profile-list/v1).
    #[command(subcommand)]
    Profiles(ProfilesCmd),
    /// A clinic's portal, read from its public configuration (org-list/v1).
    #[command(subcommand)]
    Orgs(OrgsCmd),
    /// The patient a profile's login reads (patient-list/v1).
    #[command(subcommand)]
    Patients(PatientsCmd),
    /// The patient's records as records-stream/v1, for `medrec import --from fertemr`.
    #[command(subcommand)]
    Records(RecordsCmd),
    /// The portal's files, lab report PDFs and message attachments (documents/v1).
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// One read of the portal's API as --profile, printed as the JSON it
    /// answers: `fertemr api PPLab/List`. Only the portal's list and get
    /// reads are sent; a download goes through `documents download`, and
    /// anything else is refused before a request is made.
    Api(ApiArgs),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Sign in to a clinic's portal as --profile and keep the session in
    /// fertemr's vault: encrypted files under ~/.config/fertemr/vault/,
    /// sealed with a key held in the OS keychain (auth-login/v1).
    ///
    /// The user id and password come from --stdin (two lines: user id, then
    /// password), --from-env <VAR> (the same two lines), --op-item
    /// <op://VAULT/ITEM> (its username and password fields, read with the
    /// 1Password CLI), or --browser, which opens the portal's own sign-in page
    /// in the profile's own Chrome profile and reads the session once you are
    /// signed in. Never on the command line, and never stored: only the
    /// session the portal answers is kept.
    Login(LoginCmd),
    /// Whether the profile's session is stored, and with --check whether the
    /// portal still accepts it (auth-status/v1).
    Status {
        /// Ask the portal (one read) whether the session is still good.
        #[arg(long)]
        check: bool,
    },
    /// Sign out at the portal and remove the profile's stored session, then
    /// the browser profile its --browser sign-in keeps (auth-logout/v1).
    Logout,
    /// Remove every profile's stored session and the vault's key from the
    /// keychain (auth-reset/v1). Asks first; without a terminal it needs
    /// --force (exit 6).
    Reset {
        /// Do not ask.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Args, Debug)]
pub struct LoginCmd {
    /// The clinic's portal: its host (portal-<clinic>.fertemr.com), its URL,
    /// or the short name between `portal-` and `.fertemr.com`. Needed for a
    /// profile's first sign-in; later ones reuse the profile's.
    #[arg(long, value_name = "PORTAL")]
    pub portal: Option<String>,
    /// The household's own name for the patient (`adult1`), kept in local
    /// config only. Defaults to the profile's name.
    #[arg(long, value_name = "WHO")]
    pub label: Option<String>,
    #[command(flatten)]
    pub source: SecretSourceArgs,
    /// A 1Password login item, op://VAULT/ITEM: its `username` and
    /// `password` fields are read with `op read`, one bounded call each.
    #[arg(long, value_name = "op://VAULT/ITEM", conflicts_with_all = ["stdin", "from_env", "browser"])]
    pub op_item: Option<String>,
    #[command(flatten)]
    pub browser: BrowserArgs,
    /// Replace the session already stored for the profile (sign in again
    /// once the portal has ended it).
    #[arg(long)]
    pub overwrite: bool,
}

#[derive(Subcommand, Debug)]
pub enum ProfilesCmd {
    /// The configured profiles, by name (profile-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// Add a profile: 1 to 32 lowercase letters, digits, `-` or `_`,
    /// starting with a letter or a digit (profile-add/v1).
    Add { name: String },
    /// Remove a profile with its stored session and the browser profile its
    /// --browser sign-in keeps (profile-remove/v1). Asks first; without a
    /// terminal it needs --force (exit 6).
    Remove {
        name: String,
        /// Do not ask.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum OrgsCmd {
    /// The portals the configured profiles sign in to (org-list/v1). Reads
    /// the local config only.
    #[command(visible_alias = "ls")]
    List,
    /// One clinic's portal, read from its public config.json (org-list/v1).
    /// Sends one request and no credential.
    Show {
        /// The portal: host, URL or short name.
        portal: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PatientsCmd {
    /// The profile's patient (patient-list/v1).
    #[command(visible_alias = "ls")]
    List,
}

#[derive(Subcommand, Debug)]
pub enum RecordsCmd {
    /// The patient's records as records-stream/v1: Patient, Encounter (visits),
    /// Observation and DiagnosticReport (lab results, embryology),
    /// DocumentReference and Binary (files, report PDFs, the treatment
    /// calendar), Communication (messages) and QuestionnaireResponse (intake
    /// forms).
    Export(ExportArgs),
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// The patient: its id from `fertemr patients list`, or its label. May be
    /// omitted: a login reads one patient.
    #[arg(long, value_name = "ID")]
    pub patient: Option<String>,
    /// Only what is dated on or after this day (YYYY-MM-DD). Files and
    /// embryology records carry no date of their own and are always included
    /// (the stream header lists them in since_unbounded).
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    /// Write the stream to stdout (`| medrec import --from fertemr`). The
    /// stream is the output, so --json with it is a usage error.
    #[arg(long, conflicts_with = "output")]
    pub ndjson: bool,
    /// Write the stream to DIR/records.ndjson instead (synthetic data only:
    /// real records go straight into medrec).
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum DocumentsCmd {
    /// The documents: files the clinic shared, lab report PDFs and message
    /// attachments (document-list/v1).
    #[command(visible_alias = "ls")]
    List(RangeArgs),
    /// Save one document, or `--all` of them (document-download/v1,
    /// document-download-batch/v1).
    #[command(visible_alias = "get")]
    Download(DownloadArgs),
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// A document id from `fertemr documents list`.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub id: Option<String>,
    /// Every document.
    #[arg(long)]
    pub all: bool,
    /// The file to write (`-` for stdout), or with --all the folder.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    /// Replace a file that is already there.
    #[arg(long)]
    pub overwrite: bool,
}

/// `fertemr api <PATH> [--data JSON]`.
#[derive(Args, Debug, Clone)]
pub struct ApiArgs {
    /// An endpoint under the portal's /Portal/ path, such as `PPLab/List`.
    pub path: String,
    /// The JSON body to send; `{"s": ""}` when absent, as the portal's own
    /// list calls send.
    #[arg(long)]
    pub data: Option<String>,
}
