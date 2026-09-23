//! Independently generated valid-format fixtures. These are not rhwp roundtrips.
//! Expected body strings live next to the builders so tests do not infer text
//! from a parser.

use std::io::{Cursor, Write};

use cfb::CompoundFile;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

pub const HWP_SEC0_P0: &str = "한글본문색인토큰";
pub const HWP_SEC0_P1: &str = "HWPTOKEN";
pub const HWP_SEC0_P2: &str = "🚀✨";
pub const HWP_SEC1_P0: &str = "둘째구역문단";
pub const HWP_PRVTEXT_DECOY: &str = "PrvText decoy must not be extracted";

pub const HWPX_SEC0_P0: &str = "한글본문색인토큰";
pub const HWPX_SEC0_P1: &str = "HWPXTOKEN";
pub const HWPX_CELL_00: &str = "가나다";
pub const HWPX_CELL_01: &str = "셀B";
pub const HWPX_CELL_10: &str = "🚀";
pub const HWPX_CELL_11: &str = "셀D";
pub const HWPX_SEC1_P0: &str = "둘째섹션";

pub fn expected_hwp5_body() -> String {
    [HWP_SEC0_P0, HWP_SEC0_P1, HWP_SEC0_P2, HWP_SEC1_P0].join("\n")
}

pub fn expected_hwpx_body() -> String {
    [
        HWPX_SEC0_P0,
        HWPX_SEC0_P1,
        HWPX_CELL_00,
        HWPX_CELL_01,
        HWPX_CELL_10,
        HWPX_CELL_11,
        HWPX_SEC1_P0,
    ]
    .join("\n")
}

pub fn hwp5_known_body() -> Vec<u8> {
    hwp5_document(
        &[
            &[HWP_SEC0_P0, HWP_SEC0_P1, HWP_SEC0_P2][..],
            &[HWP_SEC1_P0][..],
        ],
        Hwp5Flags {
            compressed: true,
            encrypted: false,
            distribution: false,
            prv_text: Some(HWP_PRVTEXT_DECOY),
        },
    )
}

pub fn hwp5_empty_body() -> Vec<u8> {
    hwp5_document(
        &[&[][..]],
        Hwp5Flags {
            compressed: true,
            encrypted: false,
            distribution: false,
            prv_text: None,
        },
    )
}

pub fn hwp5_encrypted_flag() -> Vec<u8> {
    hwp5_document(
        &[&[HWP_SEC0_P0][..]],
        Hwp5Flags {
            compressed: true,
            encrypted: true,
            distribution: false,
            prv_text: None,
        },
    )
}

pub fn hwp5_distribution_flag() -> Vec<u8> {
    hwp5_document(
        &[&[HWP_SEC0_P0][..]],
        Hwp5Flags {
            compressed: true,
            encrypted: false,
            distribution: true,
            prv_text: None,
        },
    )
}

pub fn hwp5_uncompressed_body() -> Vec<u8> {
    hwp5_document(
        &[&[HWP_SEC0_P0][..]],
        Hwp5Flags {
            compressed: false,
            encrypted: false,
            distribution: false,
            prv_text: None,
        },
    )
}

struct Hwp5Flags {
    compressed: bool,
    encrypted: bool,
    distribution: bool,
    prv_text: Option<&'static str>,
}

fn hwp5_document(sections: &[&[&str]], flags: Hwp5Flags) -> Vec<u8> {
    let mut header = vec![0u8; 256];
    header[..17].copy_from_slice(b"HWP Document File");
    header[31] = 0x1a;
    header[32..36].copy_from_slice(&[0, 3, 0, 5]);
    let mut attr = 0u32;
    if flags.compressed {
        attr |= 0x01;
    }
    if flags.encrypted {
        attr |= 0x02;
    }
    if flags.distribution {
        attr |= 0x04;
    }
    header[36..40].copy_from_slice(&attr.to_le_bytes());

    let mut doc_info = Vec::new();
    let mut props = vec![0u8; 26];
    props[0..2].copy_from_slice(&(sections.len() as u16).to_le_bytes());
    doc_info.extend(hwp_record(16, 0, &props));
    let mappings = vec![0u8; 32];
    doc_info.extend(hwp_record(17, 0, &mappings));

    let cursor = Cursor::new(Vec::new());
    let mut cfb = CompoundFile::create(cursor).expect("create cfb");
    write_stream(&mut cfb, "/FileHeader", &header);
    write_stream(
        &mut cfb,
        "/DocInfo",
        &maybe_deflate(&doc_info, flags.compressed),
    );
    cfb.create_storage("/BodyText").expect("BodyText storage");
    for (i, paras) in sections.iter().enumerate() {
        let raw = section_records(paras);
        let path = format!("/BodyText/Section{i}");
        write_stream(&mut cfb, &path, &maybe_deflate(&raw, flags.compressed));
    }
    if let Some(prv) = flags.prv_text {
        let utf16: Vec<u8> = prv.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        write_stream(&mut cfb, "/PrvText", &utf16);
    }
    cfb.flush().expect("flush cfb");
    cfb.into_inner().into_inner()
}

fn write_stream<F: std::io::Read + std::io::Write + std::io::Seek>(
    cfb: &mut CompoundFile<F>,
    path: &str,
    data: &[u8],
) {
    let mut stream = cfb.create_stream(path).unwrap_or_else(|_| {
        panic!("create stream {path}");
    });
    stream.write_all(data).expect("write stream");
}

fn maybe_deflate(data: &[u8], compressed: bool) -> Vec<u8> {
    if !compressed {
        return data.to_vec();
    }
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("deflate");
    enc.finish().expect("deflate finish")
}

fn hwp_record(tag_id: u16, level: u16, data: &[u8]) -> Vec<u8> {
    let size = data.len();
    let header = (u32::from(tag_id) & 0x3FF)
        | ((u32::from(level) & 0x3FF) << 10)
        | ((size as u32 & 0xFFF) << 20);
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn para_header(n_chars: u32) -> Vec<u8> {
    let mut data = vec![0u8; 22];
    data[0..4].copy_from_slice(&n_chars.to_le_bytes());
    data
}

fn utf16_le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn section_records(paragraphs: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    if paragraphs.is_empty() {
        let empty = "";
        let units = utf16_le(empty);
        out.extend(hwp_record(66, 0, &para_header(0)));
        out.extend(hwp_record(67, 1, &units));
        out.extend(hwp_record(68, 1, &[0u8; 8]));
        return out;
    }
    for text in paragraphs {
        let units = utf16_le(text);
        let n_chars = (units.len() / 2) as u32;
        out.extend(hwp_record(66, 0, &para_header(n_chars)));
        out.extend(hwp_record(67, 1, &units));
        out.extend(hwp_record(68, 1, &[0u8; 8]));
    }
    out
}

pub fn hwpx_known_body() -> Vec<u8> {
    let section0 = hwpx_section(
        &[HWPX_SEC0_P0, HWPX_SEC0_P1],
        Some(&[
            &[HWPX_CELL_00, HWPX_CELL_01][..],
            &[HWPX_CELL_10, HWPX_CELL_11][..],
        ]),
    );
    let section1 = hwpx_section(&[HWPX_SEC1_P0], None);
    pack_hwpx(&[
        ("Contents/section0.xml", section0),
        ("Contents/section1.xml", section1),
    ])
}

pub fn hwpx_empty_body() -> Vec<u8> {
    pack_hwpx(&[("Contents/section0.xml", hwpx_section(&[""], None))])
}

pub fn hwpx_encrypted_manifest() -> Vec<u8> {
    let enc = r#"<odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
  <odf:file-entry odf:full-path="Contents/header.xml">
    <odf:encryption-data>
      <odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/>
      <odf:key-derivation key-derivation-name="http://www.w3.org/2001/04/xmlenc#pbkdf2"/>
    </odf:encryption-data>
  </odf:file-entry>
</odf:manifest>"#;
    pack_hwpx_with_manifest(
        enc.as_bytes(),
        &[("Contents/section0.xml", hwpx_section(&["secret"], None))],
        Some(&[0x93u8, 0xFF, 0x00, 0x11]),
    )
}

fn pack_hwpx(sections: &[(&str, String)]) -> Vec<u8> {
    pack_hwpx_with_manifest(plain_manifest().as_bytes(), sections, None)
}

fn plain_manifest() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
  <odf:file-entry odf:full-path="/" odf:media-type="application/hwp+zip"/>
</odf:manifest>
"#
    .to_string()
}

fn pack_hwpx_with_manifest(
    manifest: &[u8],
    sections: &[(&str, String)],
    header_override: Option<&[u8]>,
) -> Vec<u8> {
    let mut items = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<opf:package xmlns:opf="http://www.hancom.co.kr/hwpml/2011/pkg">
  <opf:manifest>
    <opf:item id="header" href="Contents/header.xml" media-type="application/xml"/>
"#,
    );
    for (i, (href, _)) in sections.iter().enumerate() {
        items.push_str(&format!(
            "    <opf:item id=\"section{i}\" href=\"{href}\" media-type=\"application/xml\"/>\n"
        ));
    }
    items.push_str("  </opf:manifest>\n</opf:package>\n");

    let header = header_override.map(|b| b.to_vec()).unwrap_or_else(|| {
        br#"<?xml version="1.0" encoding="UTF-8"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>
"#
        .to_vec()
    });

    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/hwp+zip").unwrap();
        zip.start_file("version.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/application">
  <ha:version>5.0.0.0</ha:version>
</ha:HWPApplicationSetting>
"#,
        )
        .unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/>
  </rootfiles>
</container>
"#,
        )
        .unwrap();
        zip.start_file("META-INF/manifest.xml", deflated).unwrap();
        zip.write_all(manifest).unwrap();
        zip.start_file("Contents/content.hpf", deflated).unwrap();
        zip.write_all(items.as_bytes()).unwrap();
        zip.start_file("Contents/header.xml", deflated).unwrap();
        zip.write_all(&header).unwrap();
        for (href, xml) in sections {
            zip.start_file(*href, deflated).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn hwpx_section(paragraphs: &[&str], table: Option<&[&[&str]]>) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
"#,
    );
    for p in paragraphs {
        xml.push_str("  <hp:p><hp:run><hp:t>");
        xml.push_str(&xml_escape(p));
        xml.push_str("</hp:t></hp:run></hp:p>\n");
    }
    if let Some(rows) = table {
        let cols = rows.first().map(|r| r.len()).unwrap_or(0);
        xml.push_str(&format!(
            "  <hp:p><hp:run><hp:tbl rowCnt=\"{}\" colCnt=\"{}\">\n",
            rows.len(),
            cols
        ));
        for (ri, row) in rows.iter().enumerate() {
            xml.push_str("    <hp:tr>\n");
            for (ci, cell) in row.iter().enumerate() {
                xml.push_str(&format!(
                    "      <hp:tc><hp:cellAddr rowAddr=\"{ri}\" colAddr=\"{ci}\"/><hp:subList><hp:p><hp:run><hp:t>{}</hp:t></hp:run></hp:p></hp:subList></hp:tc>\n",
                    xml_escape(cell)
                ));
            }
            xml.push_str("    </hp:tr>\n");
        }
        xml.push_str("  </hp:tbl></hp:run></hp:p>\n");
    }
    xml.push_str("</hs:sec>\n");
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn zip_with_forged_uncompressed() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("Contents/content.hpf", opts).unwrap();
        zip.write_all(b"<opf:package/>").unwrap();
        zip.start_file("Contents/header.xml", opts).unwrap();
        zip.write_all(b"<hh:head/>").unwrap();
        zip.start_file("Contents/section0.xml", opts).unwrap();
        zip.write_all(b"A").unwrap();
        zip.finish().unwrap();
    }
    let mut bytes = buf.into_inner();
    let huge = (crate::limits::MAX_ZIP_UNCOMPRESSED_BYTES + 1) as u32;
    let mut i = 0;
    while i + 28 < bytes.len() {
        if bytes[i..].starts_with(b"PK\x03\x04") {
            bytes[i + 22..i + 26].copy_from_slice(&huge.to_le_bytes());
            i += 30;
        } else if bytes[i..].starts_with(b"PK\x01\x02") {
            bytes[i + 24..i + 28].copy_from_slice(&huge.to_le_bytes());
            i += 46;
        } else {
            i += 1;
        }
    }
    bytes
}

pub fn zip_with_path_escape() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("Contents/content.hpf", opts).unwrap();
        zip.write_all(b"<x/>").unwrap();
        zip.start_file("Contents/header.xml", opts).unwrap();
        zip.write_all(b"<x/>").unwrap();
        zip.start_file("../etc/passwd", opts).unwrap();
        zip.write_all(b"root:").unwrap();
        zip.finish().unwrap();
    }
    buf.into_inner()
}

pub fn truncated(bytes: &[u8]) -> Vec<u8> {
    let keep = (bytes.len() / 3).max(8);
    bytes[..keep.min(bytes.len())].to_vec()
}

pub fn corrupt_cfb() -> Vec<u8> {
    let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    b.extend_from_slice(&[0xFF; 64]);
    b
}

pub fn pdf_named_hwp() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj<<>>endobj\n%%EOF\n".to_vec()
}
