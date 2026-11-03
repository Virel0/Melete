use super::*;

const CHANGELOG_TEXT: &str = "# Changelog

Intro.

## [Unreleased]

### Fixed

- Already here, written before the files came in.

## [0.4.0] - 2026-11-02

### Added

- Old.

<!--
[Unreleased]: https://github.com/Virel0/Melete/compare/vX.Y.Z...HEAD
-->

[Unreleased]: https://github.com/Virel0/Melete/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Virel0/Melete/compare/v0.3.2...v0.4.0
";

const DESIGN_TEXT: &str = "# Design

## Decisions

| # | Decision | Why | Date |
|---|----------|-----|------|
| 1 | First | Because | 2026-01-01 |
| 7 | Seventh | Because | 2026-02-01 |

## Assumptions

- None.
";

/// A repository with the two shared files and these pending files.
fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs/decisions")).unwrap();
    std::fs::create_dir_all(dir.path().join("changes")).unwrap();
    std::fs::write(dir.path().join(CHANGELOG), CHANGELOG_TEXT).unwrap();
    std::fs::write(dir.path().join(DESIGN), DESIGN_TEXT).unwrap();
    std::fs::write(dir.path().join("changes/README.md"), "How to write one.\n").unwrap();
    for (path, text) in files {
        std::fs::write(dir.path().join(path), text).unwrap();
    }
    dir
}

#[test]
fn a_release_joins_every_file_in_section_order_and_numbers_decisions_on() {
    let dir = repo(&[
        (
            "changes/b-songs.md",
            "### Fixed\n\n- Songs fixed.\n\n### Added\n\n- Songs, with a long line\n  that runs on.\n",
        ),
        ("changes/a-home.md", "### Added\n\n- Home rows.\n"),
        (
            "docs/decisions/z-late.md",
            "Decision: Later one.\nWhy: Made second.\nDate: 2026-11-04\n",
        ),
        (
            "docs/decisions/a-early.md",
            "Decision: Earlier one, written\nover two lines.\n\nWhy: Made first.\nDate: 2026-11-03\n",
        ),
    ]);
    let said = release(dir.path(), "0.5.0", "2026-11-05").unwrap();
    assert_eq!(said, "0.5.0: 2 changelog files and 2 decisions joined");

    let changelog = std::fs::read_to_string(dir.path().join(CHANGELOG)).unwrap();
    let expected = "## [Unreleased]

## [0.5.0] - 2026-11-05

### Added

- Home rows.
- Songs, with a long line
  that runs on.

### Fixed

- Already here, written before the files came in.
- Songs fixed.

## [0.4.0] - 2026-11-02
";
    assert!(changelog.contains(expected), "{changelog}");
    assert!(changelog.contains(
        "[Unreleased]: https://github.com/Virel0/Melete/compare/v0.5.0...HEAD\n\
         [0.5.0]: https://github.com/Virel0/Melete/compare/v0.4.0...v0.5.0\n\
         [0.4.0]:"
    ));
    assert!(
        changelog.contains("compare/vX.Y.Z...HEAD"),
        "the template stays"
    );

    let design = std::fs::read_to_string(dir.path().join(DESIGN)).unwrap();
    assert!(design.contains(
        "| 7 | Seventh | Because | 2026-02-01 |\n\
         | 8 | Earlier one, written over two lines. | Made first. | 2026-11-03 |\n\
         | 9 | Later one. | Made second. | 2026-11-04 |\n\
         \n## Assumptions"
    ));

    let left: Vec<_> = pending(dir.path(), CHANGES_DIR)
        .unwrap()
        .into_iter()
        .chain(pending(dir.path(), DECISIONS_DIR).unwrap())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(dir.path().join("changes/README.md").exists());
}

#[test]
fn a_malformed_file_is_refused_and_nothing_changes() {
    for (path, text, why) in [
        ("changes/x.md", "### Improved\n\n- Hm.\n", "unknown section"),
        ("changes/x.md", "- No heading.\n", "before any section"),
        (
            "changes/x.md",
            "### Added\n\nProse, not an entry.\n",
            "not an entry",
        ),
        ("changes/x.md", "### Added\n", "no entries"),
        (
            "docs/decisions/x.md",
            "Decision: A.\nDate: 2026-11-03\n",
            "no Why:",
        ),
        (
            "docs/decisions/x.md",
            "Decision: A.\nWhy: B.\nDate: soon\n",
            "isn't YYYY-MM-DD",
        ),
        (
            "docs/decisions/x.md",
            "Decision: A | B.\nWhy: C.\nDate: 2026-11-03\n",
            "`|`",
        ),
    ] {
        let dir = repo(&[(path, text)]);
        let err = release(dir.path(), "0.5.0", "2026-11-05").unwrap_err();
        assert!(err.contains(why), "{text:?}: {err}");
        assert!(check(dir.path()).unwrap_err().contains(why));
        let changelog = std::fs::read_to_string(dir.path().join(CHANGELOG)).unwrap();
        assert_eq!(changelog, CHANGELOG_TEXT);
        assert!(dir.path().join(path).exists());
    }
}

#[test]
fn a_version_already_released_or_badly_written_is_refused() {
    let dir = repo(&[("changes/a.md", "### Added\n\n- A.\n")]);
    assert!(
        release(dir.path(), "0.4.0", "2026-11-05")
            .unwrap_err()
            .contains("already has")
    );
    assert!(release(dir.path(), "v0.5", "2026-11-05").is_err());
    assert!(release(dir.path(), "0.5.0", "05/11/2026").is_err());
}

/// The repository's own pending files are always fit to release.
#[test]
fn the_repositorys_pending_files_are_well_formed() {
    check(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
}

#[test]
fn entries_under_unreleased_that_cant_be_read_stop_the_release_rather_than_go_missing() {
    let dir = repo(&[("changes/a.md", "### Added\n\n- A.\n")]);
    let odd = CHANGELOG_TEXT.replace("### Fixed\n\n- Already here", "**Fixed**\n- Already here");
    std::fs::write(dir.path().join(CHANGELOG), &odd).unwrap();
    let err = release(dir.path(), "0.5.0", "2026-11-05").unwrap_err();
    assert!(err.contains("under Unreleased"), "{err}");
    let changelog = std::fs::read_to_string(dir.path().join(CHANGELOG)).unwrap();
    assert_eq!(changelog, odd);
}
