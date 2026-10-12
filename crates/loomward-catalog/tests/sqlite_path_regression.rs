//! Regression cases for #398. Lexical fixtures only; no host paths or filesystem effects.
use loomward_catalog::sqlite_dir;
use std::path::{Path, PathBuf};

#[test]
fn empty_and_incomplete_verbatim_prefixes_are_unchanged() {
    for path in [r"\\?\", r"\\?\C", r"\\?\C:", r"\\?\C:relative"] {
        assert_eq!(sqlite_dir(Path::new(path)), PathBuf::from(path));
    }
}

#[test]
fn sqlite_path_length_budget_counts_utf16_not_utf8() {
    let plain = format!(r"C:\{}", "数".repeat(70));
    assert!(plain.len() >= 200);
    assert!(plain.encode_utf16().count() < 200);
    assert_eq!(
        sqlite_dir(Path::new(&format!(r"\\?\{plain}"))),
        PathBuf::from(plain)
    );
}

#[test]
fn sqlite_path_length_budget_keeps_its_exact_boundary() {
    for (units, converted) in [(199, true), (200, false)] {
        let plain = format!(r"C:\{}", "a".repeat(units - 3));
        let verbatim = format!(r"\\?\{plain}");
        assert_eq!(
            sqlite_dir(Path::new(&verbatim)),
            PathBuf::from(if converted { plain } else { verbatim })
        );
    }
    // Supplementary characters consume two UTF-16 code units each.
    let verbatim = format!(r"\\?\C:\{}", "🧵".repeat(99));
    assert_eq!(sqlite_dir(Path::new(&verbatim)), PathBuf::from(verbatim));
}

#[test]
fn win32_normalised_components_keep_their_verbatim_identity() {
    for component in [
        "trailing.",
        "trailing ",
        ".",
        "..",
        "CON",
        "con.txt",
        "CON .txt",
        "NUL",
        "aux.log",
        "PRN",
        "COM1",
        "lpt9.txt",
        "COM¹",
        "LPT².log",
        "CONIN$",
        "CONOUT$",
        "alternate:stream",
        "slash/name",
    ] {
        let verbatim = format!(r"\\?\C:\{component}\state");
        assert_eq!(
            sqlite_dir(Path::new(&verbatim)),
            PathBuf::from(&verbatim),
            "component {component:?} must not be normalised by removing the prefix"
        );
    }
}

#[test]
fn ordinary_absolute_drive_paths_still_use_the_sqlite_workaround() {
    for plain in [r"C:\fixture\state", r"d:\CONSOLE\COM10\state", r"C:\数\state"] {
        assert_eq!(
            sqlite_dir(Path::new(&format!(r"\\?\{plain}"))),
            PathBuf::from(plain)
        );
    }
    for path in [r"\\?\UNC\host\share", r"\\?\Volume{fixture}\state", r"C:\state"] {
        assert_eq!(sqlite_dir(Path::new(path)), PathBuf::from(path));
    }
}

#[cfg(unix)]
#[test]
fn non_unicode_paths_are_never_lossily_rewritten() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = br"\\?\C:\fixture-".to_vec();
    bytes.push(0xff);
    let original = PathBuf::from(OsString::from_vec(bytes));
    assert_eq!(sqlite_dir(&original), original);
}
