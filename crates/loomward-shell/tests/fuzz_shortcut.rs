//! Property fuzzing of the MS-SHLLINK parser `shortcuts::parse` (docs/45 row 6, #414), on top of the
//! fixed-seed loop in `shell.rs`. Shared seeds cover local/network LinkInfo, Unicode names, IDList,
//! environment, known-folder and tracker blocks. Properties: never panics on arbitrary bytes; the
//! seeds parse; a strict prefix of a valid link is refused (no silent acceptance of a truncated file); trailing bytes after the
//! terminal block are refused; a mutated link is refused or still carries the shell-link header.
#[path = "../../../fixtures/fuzz/support.rs"]
mod support;

use loomward_shell::shortcuts::*;
use proptest::prelude::*;
use support::{apply, config, edits, few_edits};

#[path = "support/shortcuts.rs"]
mod fixtures;
use fixtures::seeds;

proptest! {
    #![proptest_config(config(3000))]

    #[test]
    fn never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2100)) {
        let _ = parse(&bytes);
    }

    #[test]
    fn seeds_parse_and_no_proper_prefix_does(name in "[ -~]{0,40}", which in 0usize..9, cut in any::<usize>()) {
        let seed = seeds(&name)[which].clone();
        prop_assert!(parse(&seed).is_ok());
        let prefix = &seed[..cut % seed.len()];
        prop_assert!(parse(prefix).is_err(), "a {}-byte prefix of a {}-byte link parsed", prefix.len(), seed.len());
    }

    #[test]
    fn trailing_bytes_after_the_terminal_block_are_refused(name in "[ -~]{0,40}", which in 0usize..9, extra in prop::collection::vec(any::<u8>(), 1..16)) {
        let mut seed = seeds(&name)[which].clone();
        seed.extend(extra);
        prop_assert!(parse(&seed).is_err());
    }

    #[test]
    fn mutated_links_are_refused_or_keep_the_header(name in "[ -~]{0,40}", which in 0usize..9, e in prop_oneof![edits(), few_edits(), few_edits()]) {
        let mutated = apply(seeds(&name)[which].clone(), &e);
        if let Ok(s) = parse(&mutated) {
            prop_assert!(mutated.len() >= 80 && mutated[..4] == [76, 0, 0, 0]);
            prop_assert_eq!(s.flags, u32::from_le_bytes(mutated[20..24].try_into().unwrap()));
            prop_assert_eq!(parse(&mutated), Ok(s));
        }
    }
}
