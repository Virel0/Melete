# Contributing to Melete

Thanks for helping. Melete manages people's media libraries, so the rules below are strict on
purpose: an existing library is never renamed, moved or deleted, a download is never imported
to the wrong title, and credentials never leave the server.

Open an issue before a large change, so the design can be agreed first. Report security
problems privately, as [`SECURITY.md`](SECURITY.md) says.

## Layout

| Path | What |
|------|------|
| `src/` | The `melete` binary: the server, its background work and its commands. Each outside service lives in one module of its own (`src/tmdb.rs`, `src/torznab.rs`, `src/deluge.rs`…) |
| `src/api/` | The HTTP API under `/api/v1`, and `openapi.json`, the contract the apps are built against |
| `src/web/` | The web UI: plain HTML, CSS and JavaScript modules, built into the binary, no build step |
| `migrations/` | SQLite migrations, compiled into the binary |
| `tests/` | Checks on the source itself (file length, comment references) and test fixtures |
| `docs/DESIGN.md` | The design, and the decisions table: what Melete does and why |
| `stems/`, `motion/` | The optional stem separator and motion cover renderer containers |
| `Dockerfile`, `compose*.yaml` | The image and how to run it |

Larger modules are split as `src/x.rs` plus `src/x/<part>.rs`, with `use super::*;` in each
part.

## Checks

Every pull request must pass:

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The tests need `ffmpeg` and `ffprobe` on the `PATH`: some make a real clip and read it. CI also
runs `cargo audit`, checks the shell scripts, refuses OpenSSL and native-tls in the dependency
tree, and reports long files and functions.

Clippy fails a function over 100 lines and a bare `unwrap()` outside tests: split long
functions into named steps, and use `expect` with the reason a value can't be missing.
`tests/source_size.rs` fails a hand-written file over 500 lines of code; split it rather than
raising its limit.

Docker or startup changes also need the container run: start it with `docker compose`, with
an override file that points every mounted folder at a scratch folder (never your real
library), check the logs for `scan finished` and that `/api/v1/libraries` answers, then stop
it. Try it with a large fake library: some failures only show up at scale.

## Rules

- **Tests are the spec.** New behaviour comes with tests that fail without it. Fix the code,
  never the check: no skipped tests, changed assertions, new ignores or looser settings unless
  the change is agreed in the pull request.
- **Migrations** are expand, migrate, contract, and a migration is never edited once released
  (existing databases check their checksums). Add a new one instead. Constraints belong in the
  database.
- **API changes** update `src/api/openapi.json` in the same pull request, including who may call
  each operation (`x-melete-access`) and which API-key scope reaches it (`x-melete-scope`).
  Tests check that documented routes exist and that the main responses match.
- **A new rule or behaviour** gets a row in the decisions table in `docs/DESIGN.md`: what and
  why.
- **Outside services** are off by default, live in one module each with timeouts, can be set
  from Settings → Outside services (`src/settings/catalogue.rs`), and say exactly what leaves
  the server. Requests carry nothing that identifies the household: no names, emails or account
  links. Tests never contact a real outside service; use fakes.
- **No telemetry**, analytics or update checks.
- **Secrets** never go into responses, logs, URLs or error messages.
- **Dependencies**: avoid new ones. A new one must be open source, maintained and free of
  telemetry, pinned with `=` in `Cargo.toml`, with `Cargo.lock` committed. Say why in the pull
  request. Docker base images are pinned by digest.
- **Comments** say why, not which issue: a comment citing an issue number fails
  `tests/comment_references.rs`. Every `pub` item has a doc comment.
- **Web UI** changes follow the existing pages: labels, keyboard use, focus, and every state
  (loading, empty, error). Pages are built from DOM nodes and text, never parsed markup.

## Pull requests

- Keep them small: one change each, ideally under about 1,500 lines.
- Branch from an up-to-date `main`. Commit messages have an imperative first line of 72
  characters at most, and a body saying why.
- Fill in the pull request template: what changed, why, and what you checked.
- Never commit `.env` files (except `.env.example`), credentials, keys or `target/`. Examples and
  test data use neutral values (`/path/to/...`, `alice`, `192.168.1.10`).

By contributing, you agree that your contributions are licensed under GPL-3.0-or-later.
