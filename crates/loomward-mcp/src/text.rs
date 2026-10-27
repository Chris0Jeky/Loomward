use serde_json::Value;
use unicode_general_category::{get_general_category, GeneralCategory};

/// Visible escapes keep untrusted names readable without letting format controls affect the host.
pub fn escape(value: &mut Value) {
    match value {
        Value::String(text) => *text = escape_string(text),
        Value::Array(items) => items.iter_mut().for_each(escape),
        Value::Object(map) => map.values_mut().for_each(escape),
        _ => {}
    }
}

pub fn escape_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        // Format characters, every control, line separators and the default-ignorable fillers and
        // variation selectors that can hide text (review of #897; cf. loomward_ask::turn::clean).
        if c == '\\' {
            out.push_str("\\\\");
        } else if get_general_category(c) == GeneralCategory::Format
            || c.is_control()
            || matches!(
                c,
                '\u{2028}'
                    | '\u{2029}'
                    | '\u{034F}'
                    | '\u{115F}'
                    | '\u{1160}'
                    | '\u{17B4}'
                    | '\u{17B5}'
                    | '\u{180B}'..='\u{180F}'
                    | '\u{3164}'
                    | '\u{FE00}'..='\u{FE0F}'
                    | '\u{FFA0}'
                    | '\u{E0100}'..='\u{E01EF}'
            )
        {
            for unit in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{unit:04X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_format_character_and_line_separator_is_visible_in_nested_text() {
        let mut value =
            json!({"rows":[{"name":"synthetic\u{202e}\u{2028}\u{85}\u{2029}\u{e0001}"}]});
        escape(&mut value);
        assert_eq!(
            value["rows"][0]["name"],
            "synthetic\\u202E\\u2028\\u0085\\u2029\\uDB40\\uDC01"
        );
        let mut hidden = json!("a\u{1b}b\nc\u{fe0f}d\u{e0100}e\u{3164}f");
        escape(&mut hidden);
        assert_eq!(hidden, "a\\u001Bb\\u000Ac\\uFE0Fd\\uDB40\\uDD00e\\u3164f");
    }

    #[test]
    fn escaping_is_injective_for_literal_escapes_and_hidden_characters() {
        // Escaping is injective, not idempotent: literal escape text must stay distinguishable.
        let names = [
            "x\u{202E}y",
            r"x\u202Ey",
            r"x\\u202Ey",
            "x\ny",
            r"x\u000Ay",
            "x\u{E0001}y",
            r"x\uDB40\uDC01y",
            "ordinary.txt",
        ];
        let escaped: std::collections::BTreeSet<_> =
            names.iter().map(|name| escape_string(name)).collect();
        assert_eq!(escaped.len(), names.len());
        assert_eq!(escape_string(r"x\u202Ey"), r"x\\u202Ey");
    }
}
