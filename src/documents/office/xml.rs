//! Streaming XML helpers over `quick-xml`. Only the five predefined entities
//! and character references are resolved; DTD entities are never expanded, so
//! entity-expansion bombs have nothing to expand.

use std::borrow::Cow;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::Reader;

/// One event with namespace prefixes stripped from names.
pub enum Xml<'a> {
    Start(BytesStart<'a>),
    Empty(BytesStart<'a>),
    End(Vec<u8>),
    Text(Cow<'a, str>),
}

pub fn local(start: &BytesStart<'_>) -> Vec<u8> {
    start.local_name().as_ref().to_vec()
}

/// Attribute by local name (prefix ignored).
pub fn attr(start: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    start
        .attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// Attribute by full qualified name (for `r:id` style keys).
pub fn attr_qualified(start: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    start
        .attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn resolve_ref(r: &BytesRef<'_>) -> Option<String> {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return Some(c.to_string());
    }
    let name = r.decode().ok()?;
    resolve_predefined_entity(&name).map(str::to_string)
}

/// Walks every event of `xml`, calling `visit`. Stops at the first error or
/// when `visit` returns `Err`.
pub fn walk<E>(
    xml: &[u8],
    mut visit: impl FnMut(Xml<'_>) -> Result<(), E>,
    malformed: impl Fn(String) -> E,
) -> Result<(), E> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|err| malformed(format!("xml: {err}")))?;
        match event {
            Event::Start(start) => visit(Xml::Start(start))?,
            Event::Empty(start) => visit(Xml::Empty(start))?,
            Event::End(end) => visit(Xml::End(end.local_name().as_ref().to_vec()))?,
            Event::Text(text) => {
                let decoded = text
                    .decode()
                    .map_err(|err| malformed(format!("xml text: {err}")))?;
                visit(Xml::Text(decoded))?
            }
            Event::CData(data) => {
                let decoded = data
                    .decode()
                    .map_err(|err| malformed(format!("xml cdata: {err}")))?;
                visit(Xml::Text(decoded))?
            }
            Event::GeneralRef(r) => {
                if let Some(text) = resolve_ref(&r) {
                    visit(Xml::Text(Cow::Owned(text)))?
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// OPC relationship targets of a `.rels` part: `Id` → resolved part path.
pub fn relationships(rels: &[u8], base_dir: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let _ = walk(
        rels,
        |event| {
            if let Xml::Start(e) | Xml::Empty(e) = event {
                if local(&e) == b"Relationship" {
                    if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                        out.insert(id, resolve_part(base_dir, &target));
                    }
                }
            }
            Ok::<(), ()>(())
        },
        |_| (),
    );
    out
}

/// Resolves an OPC target against the source part's directory.
pub fn resolve_part(base_dir: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') {
        abs.to_string()
    } else if base_dir.is_empty() {
        target.to_string()
    } else {
        format!("{base_dir}/{target}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predefined_and_char_refs_resolve_but_dtd_entities_do_not() {
        let xml = br#"<?xml version="1.0"?><!DOCTYPE a [<!ENTITY x "BOOM">]><a>1 &amp; 2 &#xAC00; &x;</a>"#;
        let mut text = String::new();
        walk(
            xml,
            |e| {
                if let Xml::Text(t) = e {
                    text.push_str(&t);
                }
                Ok::<(), String>(())
            },
            |e| e,
        )
        .unwrap();
        assert_eq!(text, "1 & 2 가 ");
    }

    #[test]
    fn part_targets_resolve_relative_and_absolute() {
        assert_eq!(
            resolve_part("xl", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_part("xl", "/xl/worksheets/s.xml"),
            "xl/worksheets/s.xml"
        );
        assert_eq!(
            resolve_part("ppt/slides", "../media/a.png"),
            "ppt/media/a.png"
        );
    }
}
