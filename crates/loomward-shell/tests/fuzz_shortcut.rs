//! Property fuzzing of the MS-SHLLINK parser `shortcuts::parse` (docs/45 row 6, #414), on top of the
//! fixed-seed loop in `shell.rs`. Seeds: the synthetic links built there (empty, local link info,
//! Unicode name). Properties: never panics on arbitrary bytes; the seeds parse; a strict prefix of a
//! valid link is refused (no silent acceptance of a truncated file); trailing bytes after the
//! terminal block are refused; a mutated link is refused or still carries the shell-link header.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_shell::shortcuts::*;
use proptest::prelude::*;
use support::{apply, config, edits, few_edits};

fn put32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn header(flags: u32) -> Vec<u8> {
    let mut b = vec![0; 76];
    put32(&mut b, 0, 76);
    b[4..20].copy_from_slice(&[1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70]);
    put32(&mut b, 20, flags);
    put32(&mut b, 24, 32);
    b
}
fn string(b: &mut Vec<u8>, s: &str) {
    let u: Vec<u16> = s.encode_utf16().collect();
    b.extend((u.len() as u16).to_le_bytes());
    for c in u {
        b.extend(c.to_le_bytes());
    }
}
fn local_info() -> Vec<u8> {
    let mut b = vec![0; 28];
    put32(&mut b, 4, 28);
    put32(&mut b, 8, 1);
    put32(&mut b, 12, 28);
    b.extend(18u32.to_le_bytes());
    b.extend(3u32.to_le_bytes());
    b.extend(123u32.to_le_bytes());
    b.extend(16u32.to_le_bytes());
    b.extend(b"V\0");
    let at = b.len();
    put32(&mut b, 16, at as u32);
    b.extend(b"Q:\\fixture\0");
    let at = b.len();
    put32(&mut b, 24, at as u32);
    b.extend(b"\\doc.txt\0");
    let size = b.len();
    put32(&mut b, 0, size as u32);
    b
}
fn finish(mut b: Vec<u8>) -> Vec<u8> {
    b.extend([0; 4]);
    b
}

fn seeds(name: &str) -> [Vec<u8>; 3] {
    let mut local = header(2);
    local.extend(local_info());
    let mut text = header(0x84);
    string(&mut text, name);
    [finish(header(0)), finish(local), finish(text)]
}

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2100)) {
        let _ = parse(&bytes);
    }

    #[test]
    fn seeds_parse_and_no_proper_prefix_does(name in "[ -~]{0,40}", which in 0usize..3, cut in any::<usize>()) {
        let seed = seeds(&name)[which].clone();
        prop_assert!(parse(&seed).is_ok());
        let prefix = &seed[..cut % seed.len()];
        prop_assert!(parse(prefix).is_err(), "a {}-byte prefix of a {}-byte link parsed", prefix.len(), seed.len());
    }

    #[test]
    fn trailing_bytes_after_the_terminal_block_are_refused(name in "[ -~]{0,40}", which in 0usize..3, extra in prop::collection::vec(any::<u8>(), 1..16)) {
        let mut seed = seeds(&name)[which].clone();
        seed.extend(extra);
        prop_assert!(parse(&seed).is_err());
    }

    #[test]
    fn mutated_links_are_refused_or_keep_the_header(name in "[ -~]{0,40}", which in 0usize..3, e in prop_oneof![edits(), few_edits(), few_edits()]) {
        let mutated = apply(seeds(&name)[which].clone(), &e);
        if let Ok(s) = parse(&mutated) {
            prop_assert!(mutated.len() >= 80 && mutated[..4] == [76, 0, 0, 0]);
            prop_assert_eq!(s.flags, u32::from_le_bytes(mutated[20..24].try_into().unwrap()));
            prop_assert_eq!(parse(&mutated), Ok(s));
        }
    }
}
