#!/usr/bin/env python3
"""Exercise document conversion in the installed server; Python is a test client only."""
import base64
import http.cookiejar
import io
import json
import pathlib
import sys
import urllib.error
import urllib.request
import zipfile
import xml.etree.ElementTree as ET

base, workspace, cookie_file, state_file, phase = sys.argv[1:]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


jar = http.cookiejar.MozillaCookieJar(cookie_file)
jar.load(ignore_discard=True, ignore_expires=True)
private = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(jar))
public = urllib.request.build_opener(NoRedirect())


def request(path, method="GET", body=None, authenticated=True, expected=200):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"Origin": base, "Content-Type": "application/json"})
    try:
        response = (private if authenticated else public).open(req, timeout=40)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        payload = response.read()
        assert response.status == expected, (method, response.status, expected)
        return payload, response.headers


def api(path, method="GET", body=None, expected=200):
    return json.loads(request(path, method, body, expected=expected)[0])


root = f"/api/v1/workspaces/{workspace}"
state_path = pathlib.Path(state_file)
if phase == "create":
    markdown = "# 설치 검증 🎉\n\n**한글 본문** [링크](https://example.com/)\n\n| 항목 | 값 |\n| --- | --- |\n| 표 | 보존 |\n"
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("설치 검증.md", markdown)
    job = api("/api/v1/import", "POST", {
        "workspaceId": workspace, "source": "markdown-zip",
        "zipBase64": base64.b64encode(archive.getvalue()).decode(),
    }, expected=201)
    assert job["status"] == "completed", job["status"]
    assert len(job["createdDocumentIds"]) == 1
    document = job["createdDocumentIds"][0]
    path = f"{root}/documents/{document}"
    body = api(path + "/body")["contentJson"]
    encoded = json.dumps(body, ensure_ascii=False)
    for token in ("한글 본문", "🎉", '"table"', '"bold"', '"link"'):
        assert token in encoded, token
    # Edit the imported live room through the normal Markdown write path.
    api(path + "/body", "PUT", {"contentMd": markdown + "\n후속 편집 저장\n"})
    body = api(path + "/body")["contentJson"]
    assert "후속 편집 저장" in json.dumps(body, ensure_ascii=False)
    for extension, mime in (("md", "text/markdown"), ("pdf", "application/pdf"),
                            ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
                            ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation")):
        payload, headers = request(path + "/" + extension)
        assert headers["Content-Type"].startswith(mime), extension
        assert headers["Cache-Control"] == "private, no-store"
        assert payload, extension
        if extension == "md":
            assert "후속 편집 저장" in payload.decode()
        elif extension == "pdf":
            assert payload.startswith(b"%PDF-") and b"%%EOF" in payload[-1024:]
        else:
            with zipfile.ZipFile(io.BytesIO(payload)) as z:
                prefix = "word/" if extension == "docx" else "ppt/slides/"
                texts = [ET.fromstring(z.read(name)) for name in z.namelist()
                         if name.startswith(prefix) and name.endswith(".xml")]
                assert any("후속 편집 저장" in "".join(tree.itertext()) for tree in texts)
        request(path + "/" + extension, authenticated=False, expected=401)
    share = api(path + "/share-links", "POST", {}, expected=201)
    token = share["url"].rsplit("/", 1)[1]
    shared_pdf, _ = request(f"/api/v1/share/{token}/pdf", authenticated=False)
    assert shared_pdf.startswith(b"%PDF-")
    api(f"{root}/share-links/{share['id']}", "DELETE")
    request(f"/api/v1/share/{token}/pdf", authenticated=False, expected=404)
    state_path.write_text(json.dumps({"document": document, "body": body, "job": job["id"]}))
    print("installed Rust import, edit, MD/PDF/DOCX/PPTX exports, public PDF and revocation: ok")
elif phase == "restart":
    state = json.loads(state_path.read_text())
    body = api(f"{root}/documents/{state['document']}/body")["contentJson"]
    assert body == state["body"]
    job = api(f"/api/v1/import/{state['job']}?workspaceId={workspace}")
    assert job["status"] == "completed"
    print("imported and edited document plus completed import job survive restart: ok")
else:
    raise ValueError("phase must be create or restart")
