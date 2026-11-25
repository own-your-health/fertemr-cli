# Contributing

1. `make verify` must pass: fmt-check, clippy with warnings denied, the
   tests, the smoke test.
2. Fixtures are synthetic. A new answer shape becomes a file in
   `tests/fixtures/portal/`. Field names, types, codes and statuses follow the
   site; everything else is invented (which items, how many, their order,
   dates, amounts and text), never a copy of a real answer. The dummy values
   are listed in `tests/fixtures/README.md`.
3. No personal data anywhere: code, docs, commit messages, pull requests,
   issues. `tests/no_personal_data.rs` enforces the shapes; review the rest.
4. A new portal call comes with its entry in `docs/api.md`, a fixture, and
   the stub's answer in `tests/common/mod.rs`. A call that is not a read
   needs a change to the read-only rule in `AGENTS.md` first.
5. Squash merges only, with an approval and green CI.
