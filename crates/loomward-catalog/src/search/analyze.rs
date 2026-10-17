//! Name-tuned token analysis (docs/46 section 5.2). The same function reads a file name and a
//! query, so a query token meets a name token on the same boundaries:
//! - every non-alphanumeric character separates tokens (the extension after the last dot is
//!   therefore its own token, and is never split further);
//! - a lowercase letter followed by an uppercase one breaks (`annualReport`);
//! - a letter next to a digit breaks (`IMG1234`, `v2`);
//! - a run of capitals ends before the capital that starts a word (`HTMLParser`).
//!
//! Tokens are lowercased one character at a time; diacritics are folded by the SQL token index
//! (`remove_diacritics 2`) but not here, so a diacritic-only mismatch costs a ranking boost and
//! never a result.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Lower,
    Upper,
    Digit,
    Other,
    Sep,
}
fn class(c: char) -> Class {
    if c.is_lowercase() {
        Class::Lower
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_numeric() {
        Class::Digit
    } else if c.is_alphanumeric() {
        Class::Other
    } else {
        Class::Sep
    }
}
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// The folded tokens of a name, in order.
pub(crate) fn tokens(name: &str) -> Vec<String> {
    // The extension is a trailing alphanumeric run of 1 to 8 characters after a dot that follows a stem.
    let ext_at = name.rfind('.').filter(|&i| {
        let ext = &name[i + 1..];
        i > 0 && (1..=8).contains(&ext.chars().count()) && ext.chars().all(char::is_alphanumeric)
    });
    let (stem, ext) = match ext_at {
        Some(i) => (&name[..i], Some(&name[i + 1..])),
        None => (name, None),
    };
    let chars: Vec<char> = stem.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let k = class(c);
        if k == Class::Sep {
            if !cur.is_empty() {
                out.push(fold(&std::mem::take(&mut cur)));
            }
            continue;
        }
        if let Some(&prev) = i.checked_sub(1).and_then(|p| chars.get(p)) {
            let p = class(prev);
            let next_lower = chars.get(i + 1).is_some_and(|&n| class(n) == Class::Lower);
            let breaks = match (p, k) {
                (Class::Sep, _) => false,
                (Class::Lower, Class::Upper) => true,
                (Class::Upper, Class::Upper) => next_lower,
                (Class::Digit, Class::Lower | Class::Upper | Class::Other)
                | (Class::Lower | Class::Upper | Class::Other, Class::Digit) => true,
                _ => false,
            };
            if breaks && !cur.is_empty() {
                out.push(fold(&std::mem::take(&mut cur)));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(fold(&cur));
    }
    if let Some(ext) = ext {
        out.push(fold(ext));
    }
    out
}

/// The distinct folded tokens of a query, at most `max`.
pub(crate) fn query_tokens(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokens(text) {
        if !out.contains(&t) {
            out.push(t);
        }
        if out.len() == max {
            break;
        }
    }
    out
}

/// The facts the ranker reads off one name against one query (all in folded text).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The whole name, or the name up to its extension, is the query.
    pub exact: bool,
    pub prefix: bool,
    /// Every query token starts some name token.
    pub token_start: bool,
    /// Every query token occurs somewhere in the name.
    pub inside: bool,
}

pub(crate) fn shape(name: &str, text: &str, qtokens: &[String]) -> Shape {
    let folded = fold(name);
    let text = fold(&text.split_whitespace().collect::<Vec<_>>().join(" "));
    let toks = tokens(name);
    let stem = folded
        .rsplit_once('.')
        .filter(|(s, _)| !s.is_empty())
        .map_or(folded.as_str(), |(s, _)| s);
    Shape {
        exact: !text.is_empty() && (folded == text || stem == text),
        prefix: !text.is_empty() && folded.starts_with(&text),
        token_start: !qtokens.is_empty()
            && qtokens
                .iter()
                .all(|q| toks.iter().any(|t| t.starts_with(q))),
        inside: !qtokens.is_empty() && qtokens.iter().all(|q| folded.contains(q.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(s: &str) -> Vec<String> {
        tokens(s)
    }
    #[test]
    fn names_split_on_separators_humps_digits_and_acronyms() {
        assert_eq!(
            t("AnnualReport2024.pdf"),
            ["annual", "report", "2024", "pdf"]
        );
        assert_eq!(t("IMG_1234.JPG"), ["img", "1234", "jpg"]);
        assert_eq!(t("HTMLParser.cs"), ["html", "parser", "cs"]);
        assert_eq!(t("README"), ["readme"]);
        assert_eq!(t("v2 final-draft"), ["v", "2", "final", "draft"]);
        assert_eq!(t("my.archive.tar.gz"), ["my", "archive", "tar", "gz"]);
        // The extension is never split: `mp3` stays one token.
        assert_eq!(t("track07.mp3"), ["track", "07", "mp3"]);
        assert_eq!(t("song.Mp3"), ["song", "mp3"]);
    }
    #[test]
    fn dotfiles_and_odd_dots_and_non_latin_names_do_not_break_it() {
        assert_eq!(t(".gitignore"), ["gitignore"]);
        assert_eq!(t("trailing."), ["trailing"]);
        assert_eq!(t("long.extensionname"), ["long", "extensionname"]);
        assert_eq!(t("λ資料 Åsa.txt"), ["λ資料", "åsa", "txt"]);
        assert!(t("---___...").is_empty());
        assert!(t("").is_empty());
    }
    #[test]
    fn query_tokens_are_distinct_and_bounded() {
        assert_eq!(
            query_tokens("Report report REPORT 2024", 8),
            ["report", "2024"]
        );
        assert_eq!(query_tokens("a b c d", 2), ["a", "b"]);
    }
    #[test]
    fn shape_reads_exact_prefix_token_start_and_inside() {
        let q = |s: &str| query_tokens(s, 8);
        let s = shape("Annual Report.pdf", "annual report", &q("annual report"));
        assert!(s.exact && s.prefix && s.token_start && s.inside);
        let s = shape("AnnualReport2024.pdf", "report", &q("report"));
        assert!(!s.exact && !s.prefix && s.token_start && s.inside);
        let s = shape("synthesis draft.txt", "thesis", &q("thesis"));
        assert!(!s.token_start && s.inside);
        let s = shape("x.txt", "", &[]);
        assert_eq!(s, Shape::default());
    }
}
