//! Narrow format sniff + XML/PDF string extraction. Not a conversion framework.

use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use zip::ZipArchive;

fn xml_tagged_text(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(gt) = after.find('>') else { break };
        let inner_start = gt + 1;
        let Some(end) = after[inner_start..].find(&close) else {
            break;
        };
        let inner = after[inner_start..inner_start + end].trim();
        if !inner.is_empty() {
            out.push(
                inner
                    .replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
            );
        }
        rest = &after[inner_start + end + close.len()..];
    }
    out
}

fn pdf_literal_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let mut s = Vec::new();
            i += 1;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'\\' if i + 1 < bytes.len() => {
                        s.push(bytes[i + 1]);
                        i += 2;
                    }
                    b'(' => {
                        depth += 1;
                        s.push(b'(');
                        i += 1;
                    }
                    b')' => {
                        depth -= 1;
                        if depth > 0 {
                            s.push(b')');
                        }
                        i += 1;
                    }
                    b => {
                        s.push(b);
                        i += 1;
                    }
                }
            }
            if let Ok(t) = String::from_utf8(s) {
                let t = t.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn cfb_note(bytes: &[u8]) -> serde_json::Value {
    let sig_ok = bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    let hwp_sig = bytes.windows(17).any(|w| w == b"HWP Document File");
    serde_json::json!({
        "container": "cfb",
        "signature_ok": sig_ok,
        "hwp_fileheader_signature": hwp_sig,
        "native_body_parser": "absent",
        "blocker": "No HWP 5.0 BodyText decoder (hwpx-js / hwp5 CLI / olefile not available). CFB magic only."
    })
}

fn zip_probe(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut texts = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        let size = file.size();
        entries.push(serde_json::json!({ "name": name, "size": size }));
        if name.ends_with(".xml") || name.ends_with(".hpf") {
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok() {
                for tag in ["w:t", "hp:t", "text"] {
                    texts.extend(xml_tagged_text(&buf, tag));
                }
            }
        }
    }
    Ok(serde_json::json!({
        "container": "zip",
        "entries": entries,
        "xml_text_tokens": texts,
    }))
}

fn classify(path: &Path, bytes: &[u8]) -> serde_json::Value {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if bytes.starts_with(b"%PDF-") {
        return serde_json::json!({
            "kind": "pdf",
            "magic": "%PDF",
            "native_tools": { "pdftotext": false, "mutool": false, "poppler": false },
            "probe": "uncompressed PDF literal strings",
            "strings": pdf_literal_strings(bytes),
            "note": "Source extract path is officeparser + local pdfjs worker, not this scanner."
        });
    }
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return serde_json::json!({
            "kind": "hwp-cfb",
            "ext": name.rsplit('.').next(),
            "cfb": cfb_note(bytes),
        });
    }
    if bytes.starts_with(b"PK") {
        match zip_probe(bytes) {
            Ok(z) => {
                let kind = if name.ends_with(".hwpx") {
                    "hwpx"
                } else if name.ends_with(".docx") {
                    "docx"
                } else {
                    "zip"
                };
                serde_json::json!({
                    "kind": kind,
                    "zip": z,
                    "note": match kind {
                        "hwpx" => "Source extractor is hwpx-js. This probe only reads ZIP XML tokens.",
                        "docx" => "Source extractor is officeparser. This probe only reads w:t from OOXML.",
                        _ => "zip",
                    }
                })
            }
            Err(error) => serde_json::json!({ "kind": "zip", "ok": false, "error": error }),
        }
    } else {
        serde_json::json!({
            "kind": "unknown",
            "head_hex": bytes.iter().take(16).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(""),
        })
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: extract-probe <file>...");
        std::process::exit(2);
    }
    let mut reports = Vec::new();
    for a in args {
        let path = Path::new(&a);
        match fs::read(path) {
            Ok(bytes) => reports.push(serde_json::json!({
                "path": a,
                "bytes": bytes.len(),
                "result": classify(path, &bytes),
            })),
            Err(e) => reports.push(serde_json::json!({
                "path": a,
                "ok": false,
                "error": e.to_string(),
            })),
        }
    }
    println!("{}", serde_json::to_string_pretty(&reports).unwrap());
}
