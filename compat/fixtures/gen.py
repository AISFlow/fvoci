#!/usr/bin/env python3
"""Generate format-valid PDF/DOCX/HWPX/HWP representatives. Public container specs only."""

from __future__ import annotations

import struct
import zipfile
from io import BytesIO
from pathlib import Path

ROOT = Path(__file__).resolve().parent
ENDOFCHAIN = 0xFFFFFFFE
FREESECT = 0xFFFFFFFF
FATSECT = 0xFFFFFFFD


def write_pdf() -> None:
    # Minimal PDF 1.4, uncompressed content stream, Helvetica.
    body_stream = b"BT /F1 12 Tf 72 720 Td (compat probe) Tj ET\n"
    objs = []

    def obj(n: int, payload: bytes) -> bytes:
        return f"{n} 0 obj\n".encode() + payload + b"\nendobj\n"

    objs.append(obj(1, b"<< /Type /Catalog /Pages 2 0 R >>"))
    objs.append(obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"))
    objs.append(
        obj(
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        )
    )
    objs.append(
        obj(
            4,
            f"<< /Length {len(body_stream)} >>\nstream\n".encode()
            + body_stream
            + b"endstream",
        )
    )
    objs.append(obj(5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"))
    header = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"
    chunks = [header]
    offsets = []
    pos = len(header)
    for o in objs:
        offsets.append(pos)
        chunks.append(o)
        pos += len(o)
    xref_off = pos
    xref = [b"xref\n0 6\n0000000000 65535 f \n"]
    for off in offsets:
        xref.append(f"{off:010d} 00000 n \n".encode())
    trailer = (
        b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n"
        + str(xref_off).encode()
        + b"\n%%EOF\n"
    )
    (ROOT / "sample.pdf").write_bytes(b"".join(chunks + xref) + trailer)


def write_docx() -> None:
    document = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>안녕 compat 🚀</w:t></w:r></w:p>
  </w:body>
</w:document>
"""
    ctypes = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>
"""
    rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>
"""
    buf = BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", ctypes)
        z.writestr("_rels/.rels", rels)
        z.writestr("word/document.xml", document)
    (ROOT / "sample.docx").write_bytes(buf.getvalue())


def write_hwpx() -> None:
    mimetype = b"application/hwp+zip"
    version = """<?xml version="1.0" encoding="UTF-8"?>
<ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/application">
  <ha:version>5.0.0.0</ha:version>
</ha:HWPApplicationSetting>
"""
    container = """<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/>
  </rootfiles>
</container>
"""
    manifest = """<?xml version="1.0" encoding="UTF-8"?>
<odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
  <odf:file-entry odf:full-path="/" odf:media-type="application/hwp+zip"/>
</odf:manifest>
"""
    content_hpf = """<?xml version="1.0" encoding="UTF-8"?>
<opf:package xmlns:opf="http://www.hancom.co.kr/hwpml/2011/pkg">
  <opf:manifest>
    <opf:item id="header" href="header.xml"/>
    <opf:item id="section0" href="section0.xml"/>
  </opf:manifest>
</opf:package>
"""
    header = """<?xml version="1.0" encoding="UTF-8"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>
"""
    section = """<?xml version="1.0" encoding="UTF-8"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p>
    <hp:run>
      <hp:t>한글본문색인토큰 HWPX</hp:t>
    </hp:run>
  </hp:p>
</hs:sec>
"""
    buf = BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("mimetype", mimetype, compress_type=zipfile.ZIP_STORED)
        z.writestr("version.xml", version, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("META-INF/container.xml", container, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("META-INF/manifest.xml", manifest, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("Contents/content.hpf", content_hpf, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("Contents/header.xml", header, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("Contents/section0.xml", section, compress_type=zipfile.ZIP_DEFLATED)
    (ROOT / "sample.hwpx").write_bytes(buf.getvalue())


def _dir_entry(
    name: str,
    obj_type: int,
    start: int,
    size: int,
    child: int = -1,
    left: int = -1,
    right: int = -1,
) -> bytes:
    out = bytearray(128)
    encoded = name.encode("utf-16le") + b"\x00\x00"
    out[: min(64, len(encoded))] = encoded[:64]
    struct.pack_into("<H", out, 64, len(name) * 2 + 2)
    out[66] = obj_type
    struct.pack_into("<iii", out, 68, left, right, child)
    struct.pack_into("<I", out, 116, start & 0xFFFFFFFF)
    struct.pack_into("<Q", out, 120, size)
    return bytes(out)


def write_hwp_cfb() -> None:
    """OLE CFB v3 with a FileHeader stream. Public MS-CFB + HWP 5.0 file signature."""
    header_payload = bytearray(256)
    header_payload[:17] = b"HWP Document File"
    header_payload[17:32] = b"\x00" * 15
    # Public HWP5: version bytes after signature; keep zeros besides signature.
    payload = bytes(header_payload)

    sector = 512
    fat = [FREESECT] * 128
    fat[0] = FATSECT  # sector 0 is the FAT
    fat[1] = ENDOFCHAIN  # directory
    fat[2] = ENDOFCHAIN  # FileHeader stream

    dir_sec = (
        _dir_entry("Root Entry", 5, ENDOFCHAIN, 0, child=1)
        + _dir_entry("FileHeader", 2, 2, len(payload))
        + _dir_entry("", 0, 0, 0)
        + _dir_entry("", 0, 0, 0)
    )
    assert len(dir_sec) == 512

    stream_sec = payload.ljust(sector, b"\x00")
    fat_sec = b"".join(struct.pack("<I", x) for x in fat)

    file_header = bytearray(512)
    file_header[0:8] = bytes.fromhex("D0CF11E0A1B11AE1")
    struct.pack_into("<HH", file_header, 24, 0x003E, 0x0003)
    struct.pack_into("<HH", file_header, 28, 0xFFFE, 9)
    struct.pack_into("<H", file_header, 32, 6)
    struct.pack_into("<I", file_header, 44, 1)  # number of FAT sectors
    struct.pack_into("<I", file_header, 48, 1)  # first directory sector
    struct.pack_into("<I", file_header, 56, 4096)
    struct.pack_into("<I", file_header, 60, ENDOFCHAIN)
    struct.pack_into("<I", file_header, 68, ENDOFCHAIN)
    # DIFAT[0] = sector 0
    struct.pack_into("<I", file_header, 76, 0)
    for i in range(1, 109):
        struct.pack_into("<I", file_header, 76 + i * 4, FREESECT)

    (ROOT / "sample.hwp").write_bytes(bytes(file_header) + fat_sec + dir_sec + stream_sec)


def main() -> None:
    write_pdf()
    write_docx()
    write_hwpx()
    write_hwp_cfb()
    for n in ["sample.pdf", "sample.docx", "sample.hwpx", "sample.hwp"]:
        p = ROOT / n
        print(f"{n} {p.stat().st_size} bytes")


if __name__ == "__main__":
    main()
