//! #428: RtlIsDosDeviceName_U documents COMn/LPTn with a digit, including zero.
//! Lexical fixtures only; do not probe or open actual devices.
use loomward_catalog::sqlite_dir;
use std::path::{Path, PathBuf};

#[test]
fn zero_numbered_port_components_preserve_the_verbatim_namespace() {
    for name in ["COM0", "LPT0", "com0.txt", "lpt0.log", "COM0 .txt"] {
        let path = format!(r"\\?\C:\{name}\state");
        assert_eq!(sqlite_dir(Path::new(&path)), PathBuf::from(&path));
    }
}

#[test]
fn multi_digit_port_like_names_still_use_the_sqlite_workaround() {
    for name in ["COM00", "LPT00", "COM10", "LPT10"] {
        let plain = format!(r"C:\{name}\state");
        let path = format!(r"\\?\{plain}");
        assert_eq!(sqlite_dir(Path::new(&path)), PathBuf::from(plain));
    }
}
