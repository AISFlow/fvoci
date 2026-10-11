//! Python `json.dumps` byte forms used by the receipts: compact
//! (`separators=(',', ':')`) for the closure digest and `indent=2` for the
//! receipt files. Both use the default `ensure_ascii=True`; object keys come
//! out sorted because `serde_json::Map` is ordered by key.

use serde_json::Value;
use std::fmt::Write;

pub fn string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

fn write_value(value: &Value, indent: Option<usize>, level: usize, out: &mut String) {
    let newline = |out: &mut String, level: usize| {
        if let Some(width) = indent {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', width * level));
        }
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            let _ = write!(out, "{number}");
        }
        Value::String(text) => string(text, out),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                write_value(item, indent, level + 1, out);
            }
            newline(out, level);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push('{');
            // Sort explicitly: `Map` is insertion-ordered if any crate enables
            // serde_json `preserve_order`.
            let mut items: Vec<_> = map.iter().collect();
            items.sort_by(|a, b| a.0.cmp(b.0));
            for (index, (key, item)) in items.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                string(key, out);
                out.push_str(if indent.is_some() { ": " } else { ":" });
                write_value(item, indent, level + 1, out);
            }
            newline(out, level);
            out.push('}');
        }
    }
}

/// `json.dumps(items, separators=(',', ':'))` for a borrowed list.
pub fn compact_list(items: &[Value]) -> String {
    let mut out = String::from("[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_value(item, None, 0, &mut out);
    }
    out.push(']');
    out
}

/// `json.dumps(value, separators=(',', ':'))`.
pub fn compact(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, None, 0, &mut out);
    out
}

/// `json.dumps(value, sort_keys=True, indent=2) + '\n'`.
pub fn indented(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, Some(2), 0, &mut out);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_ascii_escapes_match_python() {
        let mut out = String::new();
        string("a\"\\\n\r\t\u{8}\u{c}\u{1}\u{7f}é😀/", &mut out);
        assert_eq!(
            out,
            "\"a\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u007f\\u00e9\\ud83d\\ude00/\""
        );
    }

    #[test]
    fn compact_and_indented_forms_match_python() {
        let value = json!({"b": [1, "x", true, null], "a": {}, "c": [], "d": {"z": 0}});
        let items = vec![json!(1), json!({"b": 1, "a": [2]})];
        assert_eq!(compact_list(&items), compact(&Value::Array(items.clone())));
        assert_eq!(
            compact(&value),
            r#"{"a":{},"b":[1,"x",true,null],"c":[],"d":{"z":0}}"#
        );
        assert_eq!(
            indented(&value),
            "{\n  \"a\": {},\n  \"b\": [\n    1,\n    \"x\",\n    true,\n    null\n  ],\n  \"c\": [],\n  \"d\": {\n    \"z\": 0\n  }\n}\n"
        );
    }
}
