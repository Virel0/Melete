Decision: Changelog entries and decisions are written one file per pull request, in `changes/`
and `docs/decisions/`, and joined into `CHANGELOG.md` and the decisions table at release by
`release_docs`, which numbers the decisions then. `cargo test` checks the files' form.

Why: Every pull request added to the same two places, so open ones always conflicted there,
and fixing that restarted their checks though only documentation had changed; pull requests
opened together also took the same decision number.

Date: 2026-11-02
