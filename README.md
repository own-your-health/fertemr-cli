# fertemr

Unofficial; not affiliated with or endorsed by Redshift Technologies or any
clinic.

`fertemr` reads a patient's records from a clinic's patient portal on the
FERT fertility EMR (`portal-<clinic>.fertemr.com`) and hands them to the
household's medical-records system as FHIR (`records-stream/v1`, read by
`medrec import --from fertemr`). It covers lab results, visits, embryology
records, the files the clinic shared, messages and their attachments, intake
forms and the treatment calendar. It is read-only: it sends the portal's own
list, get and download reads, and nothing that changes anything there.

FERT publishes no patient API. fertemr reads the API the portal's own web
app calls, as the patient, with the patient's sign-in. `docs/api.md` records
how that was decided and every call fertemr makes.

## Install

```sh
cargo install --git https://github.com/piekstra/fertemr-cli --locked
# or download a release: fertemr-<target>.tar.gz, then `fertemr self-update` later
```

## Use

One profile per portal account:

```sh
fertemr profiles add adult1
fertemr orgs show someclinic                    # the clinic's public config.json; no sign-in
fertemr auth login --profile adult1 --portal someclinic --op-item "op://Private/Clinic portal"
fertemr auth status --profile adult1 --check    # one read: does the portal still accept the session?
fertemr patients list --profile adult1
fertemr documents list --profile adult1
fertemr documents download --all --profile adult1 -o ~/Archive/Clinic
fertemr records export --profile adult1 --ndjson | head -1    # the stream's header

# medrec runs the export itself:
medrec config set providers fertemr
medrec import --from fertemr --from-profile adult1 --patient <id> --profile <records folder>

# One read of the portal's API, printed as the JSON it answers (docs/api.md lists them):
fertemr api PPVisits/List --profile adult1
```

`--portal` takes the clinic's short name (`someclinic` for
`portal-someclinic.fertemr.com`), its host or its URL. fertemr reads the
portal's public `config.json` for the clinic's name and its API address,
and signs in only to an API on `fertemr.com` over https.

Every command that reads a portal takes `--profile <NAME>` (or
`FERTEMR_PROFILE`). There is no default profile, and a lone profile is never
picked: without one, or with one not configured, the command is exit 2 and
lists the configured profiles. `profiles`, `orgs`, `auth reset`, `info`,
`self-update` and `completions` take no `--profile`.

Every command takes `--json` and prints one schema-tagged document, except
`records export --ndjson`, whose NDJSON stream is the output (`--json` with
it is exit 2).

| Command | Output schema |
| --- | --- |
| `patients list` | `patient-list/v1` |
| `records export` | `records-stream/v1` (NDJSON) |
| `documents list` / `download` | `document-list/v1` / `document-download/v1`, `document-download-batch/v1` |
| `orgs list` / `show` | `org-list/v1` |
| `api` | `api-response/v1`: the envelope's data, as the portal answers it |
| `auth login` / `status` / `logout` / `reset` | `auth-login/v1` / `auth-status/v1` / `auth-logout/v1` / `auth-reset/v1` |
| `profiles list` / `add` / `remove` | `profile-list/v1` / `profile-add/v1` / `profile-remove/v1` |
| `info` | `cli-info/v1`: profiles `health-records/v1` and `documents/v1`, capability `login-profiles` |

Exit codes: 0 ok, 2 usage, 3 not signed in or the portal ended the session,
4 not found (a portal with no `config.json`, a document id the portal does
not list), 5 the portal's answer could not be read or it failed, 6 a
destructive command (`auth reset`, `profiles remove`) without a terminal to
ask on and without `--force`.

## Signing in

The user id and password are sent once, to the portal's sign-in, and never
stored. Only the session the portal answers is kept. They come from one of:

| Flag | Where they come from |
| --- | --- |
| `--op-item op://VAULT/ITEM` | the 1Password item's `username` and `password` fields, one `op read` each (1Password may ask to approve) |
| `--stdin` | two lines on stdin: the user id, then the password |
| `--from-env VAR` | the same two lines in an environment variable |
| `--browser` | the portal's own sign-in page, in a Chrome window fertemr opens; fertemr reads the session from the page once you are signed in |

They are never taken on the command line. A refused sign-in is exit 3,
sent once and never retried. With a session already stored for the
profile, `auth login` is exit 2 unless `--overwrite`. A later sign-in reuses
the profile's portal and its device id, so `--portal` is needed only the
first time.

The portal ends a session on its own. When it has, every reading command is
exit 3 naming the sign-in that renews it (`auth login --profile <NAME>
--overwrite`).

| What | Where | Removed by |
| --- | --- | --- |
| The session: the portal, its API address, the device id, the session id, the patient's ids and name, the sign-in time; one entry `<profile>/<portal>/<patient>` | fertemr's vault: one encrypted, owner-only (`0600`) file per entry under `~/.config/fertemr/vault/`, sealed with one key, the keychain item `vault-key` in service `piekstra.fertemr` | `fertemr auth logout --profile <NAME>`, which also signs out at the portal; `fertemr auth reset` removes every entry and the key |
| The Chrome profile `--browser` signs in through, one per profile | `~/Library/Application Support/piekstra.fertemr/browser-profile-<profile>`, owner-only (`0700`); under `$XDG_DATA_HOME` on Linux | `fertemr auth logout --profile <NAME>`, after the session |
| The profile's portal, clinic name and label | fertemr's config file | `fertemr profiles remove <NAME>`, after its session and browser profile |

## What the export carries

| Portal | FHIR |
| --- | --- |
| The signed-in patient | `Patient` |
| Visits | `Encounter`, `finished` once past, `planned` before |
| Lab results | one `Observation` per result, the value spelled as the portal spells it (`1.50` stays `1.50`; `<0.10` is a comparator); one `DiagnosticReport` per order, with its report PDF as `presentedForm` and a `Binary` |
| Embryology records | one `Observation` per embryo, with stage, grade and the freeze, transfer or discard dates as components |
| Files the clinic shared | `DocumentReference` and `Binary`; an external link is a `DocumentReference` with its URL |
| Messages in the Inbox, Sent and Archive folders | `Communication`, a reply naming the message it answers; each attachment a `DocumentReference` and `Binary`. Drafts and trashed messages stay out |
| Intake forms | `QuestionnaireResponse`, the answers under their sections |
| The treatment calendar | one `DocumentReference` and `Binary` holding the portal's JSON |

Every resource carries `meta.source` `fertemr:portal`, a stable id, and a
tag saying it was mapped from the portal. A `Binary` follows the resource
that names it. What the portal does not serve the patient (no embryology,
a list that fails, a file that is not the PDF it claims to be) is a skip in
the stream's trailer with its code, not an error. A session that ends
mid-export ends the stream incomplete (`complete: false`, exit 3), and medrec
files nothing from it.

`--since D` leaves out what is dated before D. Files, embryology records
and the patient carry no date of their own and are always included; the
header lists them in `since_unbounded`.

`-o DIR` writes the stream to `DIR/records.ndjson` instead (`0600`, under a
temporary name renamed into place). It is for a stub portal on this machine
only: against a real portal it is exit 2, and real records go straight into
medrec.

## Development

`make verify` runs fmt-check, clippy, the tests and the smoke test. The
tests are offline: a stub portal on 127.0.0.1 answers from synthetic
fixtures (`tests/fixtures/README.md`), and sessions live in an in-memory
vault. See `AGENTS.md` and `CONTRIBUTING.md`.

To try fertemr by hand on synthetic data, run the same stub as a process and
point fertemr at a scratch vault:

```sh
cargo run --example stub-portal        # prints its address, user id and password
export XDG_CONFIG_HOME=$(mktemp -d) FERTEMR_KEYCHAIN_SERVICE=piekstra.fertemr.stub
fertemr profiles add test
printf 'user@example.com\nright-password\n' | fertemr auth login --profile test --portal <address> --stdin
```

`STUB_SCENARIO` (`no-embryology`, `files-fail`, `bad-pdf`, `no-calendar`,
`logout-fails`, `expire-after=N`) makes it answer the way the tests' cases do.

## License

MIT or Apache-2.0, at your option.
