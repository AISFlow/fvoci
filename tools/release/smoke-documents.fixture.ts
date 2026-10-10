// A stand-in for the installed server's document API, for testing the
// smoke-documents client's requests and verdicts (not the product): it
// accepts a Markdown ZIP import, serves the edited body and its exports,
// and records every request. `faults` turns one response wrong at a time;
// the caller may add one between the create and restart phases.
import { readZip, writeZip } from "./zip.ts";

export type Fault =
  | "import-pending"
  | "body-missing-table"
  | "edit-lost"
  | "md-cache-public"
  | "pdf-truncated"
  | "docx-without-edit"
  | "pptx-malformed"
  | "export-without-auth"
  | "share-not-revoked"
  | "redirect-body"
  | "restart-body-changed";

export type Recorded = {
  method: string;
  path: string;
  origin: string | null;
  cookie: string | null;
  contentType: string | null;
  body: string;
};

const SESSION = "fvoci_session=s3cr3t";
const DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const PPTX = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
const encoder = new TextEncoder();

function office(prefix: string, text: string, malformed: boolean): Uint8Array {
  const xml = malformed
    ? `<a:p xmlns:a="urn:a"><a:t>${text}</a:t>`
    : `<?xml version="1.0"?><w:document xmlns:w="urn:w"><w:body><w:p><w:r><w:t>${text.slice(0, 3)}</w:t></w:r><w:r><w:t xml:space="preserve">${text.slice(3)}</w:t></w:r></w:p></w:body></w:document>`;
  const name = prefix === "word/" ? "word/document.xml" : "ppt/slides/slide1.xml";
  return writeZip([
    { name: "[Content_Types].xml", data: encoder.encode("<Types/>") },
    { name, data: encoder.encode(xml) },
  ]);
}

export function startFakeServer(faults: Set<Fault> = new Set()) {
  const requests: Recorded[] = [];
  let markdown = "";
  const json = (value: unknown, status = 200) =>
    new Response(JSON.stringify(value), {
      status,
      headers: { "content-type": "application/json" },
    });
  const contentJson = () => {
    const edited = markdown.includes("후속 편집 저장") && !faults.has("edit-lost");
    const restartChanged = faults.has("restart-body-changed");
    return {
      type: "doc",
      content: [
        { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: "설치 검증 🎉" }] },
        {
          type: "paragraph",
          content: [
            { type: "text", marks: [{ type: "bold" }], text: "한글 본문" },
            {
              type: "text",
              marks: [{ type: "link", attrs: { href: "https://example.com/" } }],
              text: "링크",
            },
          ],
        },
        ...(faults.has("body-missing-table") ? [] : [{ type: "table", content: [] }]),
        ...(edited
          ? [{ type: "paragraph", content: [{ type: "text", text: "후속 편집 저장" }] }]
          : []),
        ...(restartChanged ? [{ type: "paragraph" }] : []),
      ],
    };
  };
  let shareRevoked = false;
  const server = Bun.serve({
    hostname: "localhost",
    port: 0,
    async fetch(request) {
      const url = new URL(request.url);
      const path = url.pathname + url.search;
      const body = await request.text();
      requests.push({
        method: request.method,
        path,
        origin: request.headers.get("origin"),
        cookie: request.headers.get("cookie"),
        contentType: request.headers.get("content-type"),
        body,
      });
      const authed = request.headers.get("cookie")?.split("; ").includes(SESSION) === true;
      const doc = "/api/v1/workspaces/ws1/documents/doc1";
      if (request.method === "POST" && path === "/api/v1/import") {
        const input = JSON.parse(body) as { zipBase64: string };
        const [entry] = readZip(Buffer.from(input.zipBase64, "base64"));
        if (entry?.name !== "설치 검증.md") return json({ error: "bad zip" }, 400);
        markdown = new TextDecoder().decode(entry.data);
        const status = faults.has("import-pending") ? "pending" : "completed";
        return json({ id: "job1", status, createdDocumentIds: ["doc1"] }, 201);
      }
      if (path === "/api/v1/import/job1?workspaceId=ws1")
        return json({ id: "job1", status: "completed" });
      if (path === `${doc}/body` && request.method === "GET")
        return json({ contentJson: contentJson() });
      if (path === `${doc}/body` && request.method === "PUT") {
        markdown = (JSON.parse(body) as { contentMd: string }).contentMd;
        return json({ ok: true });
      }
      const exportMatch = /^\/api\/v1\/workspaces\/ws1\/documents\/doc1\/(md|pdf|docx|pptx)$/.exec(
        path,
      );
      if (exportMatch) {
        const extension = exportMatch[1];
        if (!authed && !faults.has("export-without-auth")) return new Response("", { status: 401 });
        const cache =
          faults.has("md-cache-public") && extension === "md" ? "public" : "private, no-store";
        const send = (data: Uint8Array | string, type: string) =>
          new Response(data, { headers: { "content-type": type, "cache-control": cache } });
        if (extension === "md") return send(markdown, "text/markdown; charset=utf-8");
        if (extension === "pdf") {
          return send(
            faults.has("pdf-truncated") ? "%PDF-1.7\n" : "%PDF-1.7\n1 0 obj\n%%EOF\n",
            "application/pdf",
          );
        }
        if (extension === "docx") {
          const text = faults.has("docx-without-edit") ? "다른 문장" : "후속 편집 저장";
          return send(office("word/", text, false), DOCX);
        }
        return send(office("ppt/slides/", "후속 편집 저장", faults.has("pptx-malformed")), PPTX);
      }
      if (path === `${doc}/share-links` && request.method === "POST") {
        return json({ id: "share1", url: "http://example.invalid/s/tok1" }, 201);
      }
      if (path === "/api/v1/workspaces/ws1/share-links/share1" && request.method === "DELETE") {
        shareRevoked = !faults.has("share-not-revoked");
        return json({});
      }
      if (path === "/api/v1/share/tok1/pdf") {
        if (faults.has("redirect-body") && !shareRevoked) {
          return new Response("", { status: 302, headers: { location: "/elsewhere" } });
        }
        if (shareRevoked) return new Response("", { status: 404 });
        return new Response("%PDF-1.7\n%%EOF\n", {
          headers: { "content-type": "application/pdf" },
        });
      }
      if (path === "/elsewhere") return new Response("%PDF-1.7\n%%EOF\n");
      return new Response("not found", { status: 404 });
    },
  });
  const base = `http://localhost:${String(server.port)}`;
  return { server, base, requests, session: SESSION };
}

// A curl-written jar: the HttpOnly session cookie for the dotless host plus
// an unrelated cookie for another domain.
export function cookieJarText(host = "localhost"): string {
  return [
    "# Netscape HTTP Cookie File",
    "# https://curl.se/docs/http-cookies.html",
    "# This file was generated by libcurl! Edit at your own risk.",
    "",
    `#HttpOnly_${host}\tFALSE\t/\tFALSE\t0\tfvoci_session\ts3cr3t`,
    ".other.example\tTRUE\t/\tFALSE\t0\tfvoci_session\twrong",
    "",
  ].join("\n");
}
