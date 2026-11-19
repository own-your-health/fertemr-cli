# Security

Report a vulnerability privately through GitHub's "Report a vulnerability"
(Security tab), not in an issue.

## What fertemr touches

- Contacts no server except GitHub, for `self-update`, and the clinic's
  portal: its public `config.json` on `https://portal-<clinic>.fertemr.com`,
  then the API that file names, only when that API is on a `fertemr.com`
  host over https. Every request is one of the portal's reads, a sign-in or
  a sign-out (`docs/api.md`), sent once, never retried, with a 60-second
  limit, following only redirects that stay on https. `fertemr api` is held
  to the same reads, checked before anything is sent, and sends no download.
- Reads the user id and password once per sign-in from stdin, a named
  environment variable, or a 1Password item (`op read`, one bounded call per
  field), or not at all with `--browser`, where they are typed into the
  portal's own page. They are sent to the portal's sign-in only, and never
  stored, logged or put on a command line.
- Stores one credential per profile, the portal's session: the device id,
  the session id, the API address, the patient's ids and name and the
  sign-in time, as one vault entry. The vault (`pk_cli_auth::vault`) is
  encrypted owner-only files under `~/.config/fertemr/vault/`, with one key in
  the keychain (service `piekstra.fertemr`, account `vault-key`).
- Keeps one Chrome profile per fertemr profile for `auth login --browser`:
  `~/Library/Application Support/piekstra.fertemr/browser-profile-<profile>`
  (under `$XDG_DATA_HOME` on Linux), owner-only (`0700`). It holds the
  portal's site storage, including its session. `--fresh-profile` signs in
  without it.
- Writes only where asked: `documents download` (owner-only `0600` files,
  never replacing one without `--overwrite`, each checked against what its
  name says it is) and `records export -o` (a stub portal on this machine
  only).
- `auth logout --profile <NAME>` signs out at the portal, then removes the
  profile's session and its Chrome profile (a portal that cannot be reached
  is a warning; the session is removed anyway). `profiles remove <NAME>`
  removes the session, the Chrome profile and the profile without contacting
  the portal, and `auth reset` every session and the vault key; the portal
  ends those sessions on its own.
- The config file holds the profile names and each one's portal, clinic name
  and label.

## What fertemr never does

- Writes to the portal: no message is sent or marked read, no form saved, no
  terms accepted.
- Sends a record or its contents anywhere but stdout or a file you name.
