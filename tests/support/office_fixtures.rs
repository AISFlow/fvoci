//! Office documents built in tests (no binary fixtures in git): minimal but
//! structurally real OOXML / ODF packages and a hand-written PDF. Shared by
//! the office unit tests and the import / attachment integration suites.
#![allow(dead_code)]

use std::io::Write;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        for (name, data) in files {
            // ODF requires a stored `mimetype` first entry.
            let method = if *name == "mimetype" {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            };
            let options = SimpleFileOptions::default().compression_method(method);
            zip.start_file(*name, options).expect("zip entry");
            zip.write_all(data).expect("zip write");
        }
        zip.finish().expect("zip finish");
    }
    buf
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

/// DOCX: a `Heading1` title, body paragraphs (the first with a bold run),
/// one list item and a 2x2 table.
pub fn docx(title: &str, paragraphs: &[&str]) -> Vec<u8> {
    let mut body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>"#,
        xml_escape(title)
    );
    for (i, text) in paragraphs.iter().enumerate() {
        let rpr = if i == 0 { "<w:rPr><w:b/></w:rPr>" } else { "" };
        body.push_str(&format!(
            r#"<w:p><w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
            xml_escape(text)
        ));
    }
    body.push_str(r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>list item</w:t></w:r></w:p>"#);
    body.push_str(r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>h1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>h2</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>c1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>c2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#);
    // An alternate rendering must not duplicate text.
    body.push_str(r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:t>choice</w:t></mc:Choice><mc:Fallback><w:t>fallback</w:t></mc:Fallback></mc:AlternateContent></w:r></w:p>"#);
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W_NS}><w:body>{body}<w:p/></w:body></w:document>"#
    );
    zip_of(&[
        ("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#),
        ("_rels/.rels", br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#),
        ("word/document.xml", document.as_bytes()),
    ])
}

/// PPTX with one slide per `(title, body)`, listed in reverse part order in
/// `presentation.xml` so ordering must follow the slide id list.
pub fn pptx(slides: &[(&str, &str)]) -> Vec<u8> {
    let a_ns = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ids = String::new();
    let mut rels = String::new();
    for (i, (title, text)) in slides.iter().enumerate() {
        let part = slides.len() - i; // slide1.xml is the last slide
        ids.push_str(&format!(
            r#"<p:sldId id="{}" r:id="rId{}"/>"#,
            256 + i,
            i + 10
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{part}.xml"/>"#,
            i + 10
        ));
        let slide = format!(
            r#"<?xml version="1.0"?><p:sld {a_ns}><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:rPr lang="ko-KR" b="1"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            xml_escape(title),
            xml_escape(text)
        );
        files.push((format!("ppt/slides/slide{part}.xml"), slide.into_bytes()));
    }
    let presentation = format!(
        r#"<?xml version="1.0"?><p:presentation {a_ns}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#
    );
    let rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    files.push(("ppt/presentation.xml".into(), presentation.into_bytes()));
    files.push(("ppt/_rels/presentation.xml.rels".into(), rels.into_bytes()));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    zip_of(&refs)
}

/// XLSX with one sheet named `sheet` holding `rows` (shared strings for text,
/// inline numbers otherwise).
pub fn xlsx(sheet: &str, rows: &[&[&str]]) -> Vec<u8> {
    let mut strings: Vec<String> = Vec::new();
    let mut data = String::new();
    for (r, row) in rows.iter().enumerate() {
        data.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, value) in row.iter().enumerate() {
            let col = (b'A' + c as u8) as char;
            if value.parse::<f64>().is_ok() {
                data.push_str(&format!(r#"<c r="{col}{}"><v>{value}</v></c>"#, r + 1));
            } else {
                strings.push(value.to_string());
                data.push_str(&format!(
                    r#"<c r="{col}{}" t="s"><v>{}</v></c>"#,
                    r + 1,
                    strings.len() - 1
                ));
            }
        }
        data.push_str("</row>");
    }
    let shared: String = strings
        .iter()
        .map(|s| format!("<si><t>{}</t></si>", xml_escape(s)))
        .collect();
    let ns = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook {ns}><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        xml_escape(sheet)
    );
    let sheet_xml = format!(
        r#"<?xml version="1.0"?><worksheet {ns}><sheetData>{data}</sheetData></worksheet>"#
    );
    let shared_xml = format!(r#"<?xml version="1.0"?><sst {ns}>{shared}</sst>"#);
    zip_of(&[
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#),
        ("xl/worksheets/sheet1.xml", sheet_xml.as_bytes()),
        ("xl/sharedStrings.xml", shared_xml.as_bytes()),
    ])
}

const ODF_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#;

fn odf(mimetype: &str, body: &str) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODF_NS}><office:automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style></office:automatic-styles><office:body>{body}</office:body></office:document-content>"#
    );
    zip_of(&[
        ("mimetype", mimetype.as_bytes()),
        ("content.xml", content.as_bytes()),
    ])
}

/// ODT: heading, paragraphs (first with a bold span and `text:s`), a
/// footnote that must not leak, and a list.
pub fn odt(title: &str, paragraphs: &[&str]) -> Vec<u8> {
    let mut body = format!(
        r#"<office:text><text:h text:outline-level="1">{}</text:h>"#,
        xml_escape(title)
    );
    for (i, text) in paragraphs.iter().enumerate() {
        if i == 0 {
            body.push_str(&format!(
                r#"<text:p><text:span text:style-name="T1">{}</text:span><text:s text:c="2"/>end<text:note><text:note-body><text:p>footnote</text:p></text:note-body></text:note></text:p>"#,
                xml_escape(text)
            ));
        } else {
            body.push_str(&format!("<text:p>{}</text:p>", xml_escape(text)));
        }
    }
    body.push_str("<text:list><text:list-item><text:p>item one</text:p></text:list-item></text:list></office:text>");
    odf("application/vnd.oasis.opendocument.text", &body)
}

/// ODP with one page per `(title, body)`.
pub fn odp(pages: &[(&str, &str)]) -> Vec<u8> {
    let mut body = String::from("<office:presentation>");
    for (i, (title, text)) in pages.iter().enumerate() {
        body.push_str(&format!(
            r#"<draw:page draw:name="p{i}"><draw:frame presentation:class="title"><draw:text-box><text:p>{}</text:p></draw:text-box></draw:frame><draw:frame presentation:class="outline"><draw:text-box><text:p>{}</text:p></draw:text-box></draw:frame><presentation:notes><draw:frame><draw:text-box><text:p>speaker notes</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>"#,
            xml_escape(title),
            xml_escape(text)
        ));
    }
    body.push_str("</office:presentation>");
    odf("application/vnd.oasis.opendocument.presentation", &body)
}

/// ODS with one sheet; the last row carries a huge `number-columns-repeated`
/// empty run (as real sheets do) that must not expand.
pub fn ods(sheet: &str, rows: &[&[&str]]) -> Vec<u8> {
    let mut body = format!(
        r#"<office:spreadsheet><table:table table:name="{}">"#,
        xml_escape(sheet)
    );
    for row in rows {
        body.push_str("<table:table-row>");
        for value in *row {
            body.push_str(&format!(
                "<table:table-cell><text:p>{}</text:p></table:table-cell>",
                xml_escape(value)
            ));
        }
        body.push_str(
            r#"<table:table-cell table:number-columns-repeated="16384"/></table:table-row>"#,
        );
    }
    body.push_str(r#"<table:table-row table:number-rows-repeated="1048000"><table:table-cell table:number-columns-repeated="16384"/></table:table-row></table:table></office:spreadsheet>"#);
    odf("application/vnd.oasis.opendocument.spreadsheet", &body)
}

/// Single-page PDF (Helvetica, WinAnsi) with one text block per line, far
/// enough apart to become separate paragraphs.
pub fn pdf(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 12 Tf\n");
    for (i, line) in lines.iter().enumerate() {
        let escaped = line
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        content.push_str(&format!(
            "1 0 0 1 72 {} Tm ({escaped}) Tj\n",
            720 - (i as i32) * 60
        ));
    }
    content.push_str("ET\n");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A DOCX whose `word/document.xml` inflates past the 200 MiB zip budget
/// from a few hundred KiB of deflate.
pub fn docx_bomb() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(false);
        zip.start_file("word/document.xml", options).unwrap();
        let chunk = vec![b' '; 1 << 20];
        for _ in 0..210 {
            zip.write_all(&chunk).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}
