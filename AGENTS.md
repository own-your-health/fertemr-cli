# AGENTS.md — fertemr-cli

`fertemr` is a read-only CLI for patient portals on the FERT fertility EMR
(`portal-<clinic>.fertemr.com`). It signs in as the patient, reads the API
the portal's own web app calls, and emits FHIR for the household's
medical-records system (`records-stream/v1`, `medrec import --from
fertemr`). It conforms to piekstra-cli/1 and the `health-records/v1` and
`documents/v1` profiles, on the `pk-cli-*` crates from
[cli-common](https://github.com/piekstra/cli-common) and `pk-med-records`
from [medical-common](https://github.com/piekstra/medical-common), which
owns the records stream, the export file and the profile's DTOs. Every
`pk-cli-*` stays on the cli-common tag medical-common pins.

## Layout

| File | Owns |
|---|---|
| `src/cli.rs` | the clap tree |
| `src/app.rs` | the commands, argument checks, output; `Env` (where sessions are kept, and whether the run is hermetic) and `run`, which the in-process tests call |
| `src/auth.rs` | `auth login` (stdin, environment, 1Password or browser), `status`, `logout`, `reset`; `LoginSettings`, the profile's portal, clinic and label in config |
| `src/session.rs` | `SessionStore`: a profile's session in the vault, keyed `<profile>/<portal>/<patient>`; the only code that knows where it lives |
| `src/portal.rs` | the portal: `PortalAddress`, `discover` (config.json), `Api` (sign-in, reads, downloads), `check_read`, and how answers map to exit codes |
| `src/model.rs` | the portal's answers, every field optional and lenient about its JSON type |
| `src/pull.rs` | `Reader`, the documents listing and the export: every list in order, skips, `--since` |
| `src/fhir.rs` | the FHIR mapping, by hand |
| `docs/api.md` | the route decision, every call, and the traps |

## Medical data rules

These come from the household's plan and are not negotiable.

- The repository carries no personal data, in code, fixtures, docs, commits,
  pull requests or issues: no names, clinics the household uses, ids or real
  answers. Fixtures are synthetic (`tests/fixtures/README.md`);
  `tests/no_personal_data.rs` scans every tracked file and checks every
  identity field in the fixtures against its dummy value.
- Records and their contents are never sent to an outside service: no
  health-data aggregator, cloud OCR or other company's API. A Claude session
  is not one of these (the household's decision, 2026-11-17). A session may
  read the household's real records when the work needs it (debugging a real
  export, mapping the portal with real answers); real data stays out of
  tracked files, commits, pull requests and issues, and out of messages to
  other sessions unless they need it. A raw capture is deleted from `/tmp`
  once it has served.
- `records export -o DIR` is refused for a real portal (only a stub on this
  machine, `http://`, may write a file); real records go straight into
  medrec through a pipe.
- No FHIR, SMART or DICOM libraries, no aggregators, no one else's
  integration. The household's own crates (cli-common, medical-common) are
  not third party. General-purpose crates are fine.
- Read-only. fertemr sends only the portal's reads (`portal::READS`); a
  write would need SPEC §1.3's confirmation gate and a change to this
  paragraph in the same commit.
- No credential, session id or cookie is printed, logged or put on a command
  line. The password is never stored.

## Profiles and sessions

A profile is cli-common's (`pk_cli_config::profile`, DESIGN.md §2.5):
configured in fertemr's config (`profiles add`), resolved by
`profile::resolve` (missing, malformed or unknown is exit 2 listing the
configured names; a lone profile is never picked). A profile's settings
(portal, clinic name, label) are saved by its first successful sign-in.

A session is one vault entry (`pk_cli_auth::vault::Vault::for_binary`: one
keychain item, `vault-key` in `piekstra.fertemr`, and an encrypted file per
entry under `~/.config/fertemr/vault/`). It holds the device id the sign-in
sent; the portal binds the session to it, and later sign-ins of the profile
reuse it. `profiles remove` removes the session and the browser profile
before the profile; `auth reset` is `Vault::clear`.

## The portal

Read `docs/api.md` before changing a call. Every call is one request, never
retried, 60 seconds at most, following only secure redirects. Errors arrive
as HTTP 500 with an envelope; `portal::Api` reads its text to tell an ended
session (exit 3) from a list not offered (exit 4) from anything else (exit
5). A list that fails during an export is a skip in the trailer; an ended
session ends the stream incomplete.

Map the portal by driving it signed in (the `browser-explore` loop) and
reading the app's own traffic. Be gentle: one request at a time, no bulk
crawling.

## Tests are offline, always

`cargo test` reads no real keychain item or browser profile, contacts
nothing beyond loopback, and reads no real record.

- `tests/common/mod.rs`: a stub portal on 127.0.0.1, answering from
  `tests/fixtures/portal/`, holding one session bound to its device id, and
  logging every request.
- `tests/e2e.rs`: every command in process (`app::run`) against the stub,
  with sessions in `Vault::new(MemoryKeys, dir)`: sign-in, export, skips,
  an ended session, documents, the passthrough's refusals, sign-out, reset.
  It asserts no request outside the reads is ever sent.
- `tests/cli_surface.rs`: the binary, with `FERTEMR_KEYCHAIN_SERVICE` set to
  an empty service and a per-test `XDG_CONFIG_HOME`; only paths that store no
  session: every `--help`, exit codes, the JSON error, `info`, completions,
  `NO_COLOR`, a refused sign-in against the stub.
- `tests/fixture_shapes.rs`: the fixtures' keys against the real answers'
  keys, and the models over the fixtures.
- `tests/no_personal_data.rs`.

`examples/stub-portal.rs` runs the same stub as a process, for driving the
installed binary by hand (README § Development).

`make verify` = fmt-check, clippy, test, smoke.
