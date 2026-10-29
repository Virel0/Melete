## What and why

<!-- What this changes, and why. Link the issue it closes: "Closes #…". -->

## How it was checked

<!-- The tests added (and that they fail without the change), and anything run by hand. -->

- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` pass
- [ ] API changes are in `src/api/openapi.json`
- [ ] A new rule or behaviour has a row in the decisions table of `docs/DESIGN.md`
- [ ] Database changes are new migrations only (none released is edited)
- [ ] Anything sent outside the server is off by default and said in Settings → Outside services
- [ ] No secrets, personal names, real paths or addresses in code, tests, commits or this text
