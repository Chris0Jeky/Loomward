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
        if get_general_category(c) == GeneralCategory::Format
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{85}')
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
        let once = value.clone();
        escape(&mut value);
        assert_eq!(value, once);
    }
}
