# Decisions waiting for their number

A pull request with a new rule or behaviour adds its decision as a file here instead of a row
in the decisions table of `docs/DESIGN.md`, so pull requests never clash over the table's last
line or its next number. Name it after the change, and give three fields, each running on until
the next (lines are joined with spaces, and `|` can't be used):

```markdown
Decision: What Melete does now, in a sentence or a few.

Why: What made that the right call.

Date: 2026-11-03
```

A release numbers them on from the table's last row, in date order, adds their rows and removes
the files (`cargo run --bin release_docs -- <version> <YYYY-MM-DD>`). `cargo test` checks every
file here. Amending a decision already in the table is still done there. This README isn't
joined.
