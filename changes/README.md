# Changelog entries waiting for a release

Each pull request with a change worth a line in `CHANGELOG.md` adds its own file here instead
of editing the changelog, so two pull requests never change the same lines. Name it after the
change (`soulseek-albums.md`) and write the entries under Keep a Changelog's headings: Added,
Changed, Deprecated, Removed, Fixed or Security. An entry starts with `- `, and its further
lines are indented two spaces:

```markdown
### Added

- Albums from Soulseek in Add new, ranked with MusicBrainz's by how close each is to the
  words typed.
```

A release joins them under the new version and removes the files:
`cargo run --bin release_docs -- <version> <YYYY-MM-DD>`. `cargo test` checks every file here.
This README isn't joined.
