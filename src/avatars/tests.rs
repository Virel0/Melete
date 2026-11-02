use super::*;

/// A PNG's signature and a few bytes.
pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really the rest of a picture";

/// Melete's own pictures and a gallery folder with two categories, one of them empty, plus
/// what shouldn't be listed: a hidden file, a text file, a file outside every category.
pub(crate) fn gallery(dir: &Path) -> Gallery {
    let official = dir.join("official");
    let folder = dir.join("gallery");
    for d in [&official, &folder.join("Netflix"), &folder.join("Empty")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(official.join("melete-1.png"), PNG).unwrap();
    std::fs::write(official.join("README.md"), "about").unwrap();
    std::fs::write(folder.join("Netflix").join("b.png"), PNG).unwrap();
    std::fs::write(folder.join("Netflix").join("a.png"), PNG).unwrap();
    std::fs::write(folder.join("Netflix").join(".hidden.png"), PNG).unwrap();
    std::fs::write(folder.join("loose.png"), PNG).unwrap();
    std::fs::write(dir.join("secret.png"), PNG).unwrap();
    Gallery {
        official: Some(official),
        folder: Some(folder),
    }
}

#[test]
fn the_gallery_lists_melete_first_then_its_folders() {
    let dir = tempfile::tempdir().unwrap();
    let g = gallery(dir.path());
    let names: Vec<(String, String, usize)> = g
        .categories()
        .into_iter()
        .map(|c| (c.id, c.name, c.count))
        .collect();
    assert_eq!(
        names,
        [
            ("official".into(), "Melete".into(), 1),
            ("gallery/Netflix".into(), "Netflix".into(), 2)
        ]
    );
    let (total, page) = g.pictures("gallery/Netflix", 0, 1).unwrap();
    assert_eq!((total, page), (2, vec!["gallery/Netflix/a.png".to_owned()]));
    assert!(g.pictures("gallery/Nope", 0, 10).is_none());
    assert!(g.file("official/melete-1.png").is_some());
    assert!(g.file("gallery/Netflix/a.png").is_some());
}

#[test]
fn nothing_outside_the_gallery_is_reached() {
    let dir = tempfile::tempdir().unwrap();
    let g = gallery(dir.path());
    for id in [
        "gallery/../secret.png",
        "gallery/Netflix/../../secret.png",
        "gallery/Netflix/..",
        "gallery/..",
        "official/../secret.png",
        "gallery/Netflix/.hidden.png",
        "official/README.md",
        "gallery/loose.png",
        "gallery//a.png",
        "/etc/passwd",
        "elsewhere/Netflix/a.png",
    ] {
        assert!(g.file(id).is_none(), "{id}");
    }
    assert!(g.pictures("gallery/..", 0, 10).is_none());
    assert!(g.pictures("../gallery", 0, 10).is_none());
}

#[cfg(unix)]
#[test]
fn a_link_in_the_gallery_isnt_followed() {
    let dir = tempfile::tempdir().unwrap();
    let g = gallery(dir.path());
    let link = dir.path().join("gallery").join("Netflix").join("c.png");
    std::os::unix::fs::symlink(dir.path().join("secret.png"), &link).unwrap();
    assert!(g.file("gallery/Netflix/c.png").is_none());
    assert_eq!(g.pictures("gallery/Netflix", 0, 10).unwrap().0, 2);
}
