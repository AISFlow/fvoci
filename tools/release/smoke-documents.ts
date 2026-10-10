// Document conversion in an installed server, driven over its HTTP API by
// scripts/release-smoke.sh (and scripts/install-smoke.sh once it switches
// from scripts/install-smoke-documents.py).
//
//   bun tools/release/smoke-documents.ts BASE_URL WORKSPACE_ID COOKIE_JAR STATE_FILE create|restart
//
// create: Markdown ZIP import, a Markdown edit of the imported live room,
// MD/PDF/DOCX/PPTX exports (each refused without a session), a public PDF
// share and its revocation; the result goes to STATE_FILE.
// restart: the imported, edited body and the completed import job survive.
// Redirects are never followed; any failed check exits 1.
import { readFileSync, writeFileSync } from "node:fs";
import { ReleaseError, decodeUtf8, dumpJson, isRecord, jsonEqual } from "./py.ts";
import { xmlText } from "./xml-text.ts";
import { openZip, writeZip } from "./zip.ts";

const TIMEOUT_MS = 40_000;
const encoder = new TextEncoder();

function check(condition: boolean, message: string): asserts condition {
  if (!condition) throw new ReleaseError(message);
}

type Cookie = { domain: string; path: string; name: string; value: string };

// The curl cookie jar (Netscape format). curl writes HttpOnly cookies with a
// "#HttpOnly_" prefix; those must be read, not skipped as comments.
export function parseCookieJar(text: string, file: string): Cookie[] {
  const lines = text.split("\n");
  check(
    /#( Netscape)? HTTP Cookie File/.test(lines[0] ?? ""),
    `${file} does not look like a Netscape format cookies file`,
  );
  // domain -> path -> name, each level in first-seen order; a later line for
  // the same cookie replaces its value in place.
  const cookies = new Map<string, Map<string, Map<string, Cookie>>>();
  for (let line of lines.slice(1)) {
    if (line.startsWith("#HttpOnly_")) line = line.slice("#HttpOnly_".length);
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#") || trimmed.startsWith("$")) continue;
    const fields = line.replace(/\r$/, "").split("\t");
    check(fields.length === 7, `invalid Netscape format cookies file ${file}: ${line}`);
    const [domain = "", domainSpecified, path = "", , , rawName = "", rawValue = ""] = fields;
    check(
      (domainSpecified === "TRUE") === domain.startsWith("."),
      `invalid Netscape format cookies file ${file}: ${line}`,
    );
    const [name, value] = rawName === "" ? [rawValue, "None"] : [rawName, rawValue];
    const paths = cookies.get(domain) ?? new Map<string, Map<string, Cookie>>();
    cookies.set(domain, paths);
    const names = paths.get(path) ?? new Map<string, Cookie>();
    paths.set(path, names);
    names.set(name, { domain, path, name, value });
  }
  return [...cookies.values()].flatMap((paths) =>
    [...paths.values()].flatMap((names) => [...names.values()]),
  );
}

// The session cookie header for BASE_URL's host: cookies set for exactly that
// host (a leading dot ignored), the last one per name.
export function cookieHeader(jar: Cookie[], base: string, file: string): string {
  const host = new URL(base).hostname.replace(/^\[(.*)\]$/, "$1").toLowerCase();
  const byName = new Map<string, string>();
  for (const cookie of jar) {
    if (cookie.domain.replace(/^\.+/, "") === host) byName.set(cookie.name, cookie.value);
  }
  check(byName.has("fvoci_session"), `no fvoci_session cookie for ${host} in ${file}`);
  return [...byName].map(([name, value]) => `${name}=${value}`).join("; ");
}

type Response = { status: number; body: Uint8Array; headers: Headers };

async function readBody(response: globalThis.Response, rearm: () => void): Promise<Uint8Array> {
  if (!response.body) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  const reader = response.body.getReader();
  for (;;) {
    rearm();
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  return Buffer.concat(chunks);
}

export class Client {
  constructor(
    readonly base: string,
    private readonly cookie: string,
  ) {}

  // Each wait for the server (headers, then every body chunk) gets the full
  // timeout.
  async request(
    path: string,
    { method = "GET", body, authenticated = true, expected = 200 }: RequestOptions = {},
  ): Promise<Response> {
    const headers: Record<string, string> = {
      Origin: this.base,
      "Content-Type": "application/json",
      "Accept-Encoding": "identity",
    };
    if (authenticated) headers.Cookie = this.cookie;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const rearm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        controller.abort(
          new Error(`${method} ${path}: no response within ${String(TIMEOUT_MS)} ms`),
        );
      }, TIMEOUT_MS);
    };
    rearm();
    try {
      const response = await fetch(this.base + path, {
        method,
        headers,
        body: body === undefined ? undefined : encoder.encode(dumpJson(body)),
        redirect: "manual",
        signal: controller.signal,
      });
      const payload = await readBody(response, rearm);
      check(
        response.status === expected,
        `${method} ${path}: HTTP ${String(response.status)}, expected ${String(expected)}`,
      );
      return { status: response.status, body: payload, headers: response.headers };
    } finally {
      clearTimeout(timer);
    }
  }

  async json(path: string, options: RequestOptions = {}): Promise<unknown> {
    const { body } = await this.request(path, options);
    return JSON.parse(decodeUtf8(body));
  }

  async api(path: string, options: RequestOptions = {}): Promise<Record<string, unknown>> {
    const value = await this.json(path, options);
    check(isRecord(value), `${options.method ?? "GET"} ${path}: response is not a JSON object`);
    return value;
  }
}

type RequestOptions = {
  method?: string;
  body?: unknown;
  authenticated?: boolean;
  expected?: number;
};

const MARKDOWN =
  "# 설치 검증 🎉\n\n**한글 본문** [링크](https://example.com/)\n\n| 항목 | 값 |\n| --- | --- |\n| 표 | 보존 |\n";
const EDIT = "후속 편집 저장";
const EXPORTS: Array<[string, string]> = [
  ["md", "text/markdown"],
  ["pdf", "application/pdf"],
  ["docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  ["pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"],
];

function startsWith(bytes: Uint8Array, prefix: string): boolean {
  return Buffer.from(bytes).subarray(0, prefix.length).toString("latin1") === prefix;
}

export function isPdf(bytes: Uint8Array): boolean {
  return startsWith(bytes, "%PDF-") && Buffer.from(bytes).subarray(-1024).includes("%%EOF");
}

// Office part names are plain relative paths: no empty, "." or ".." segment,
// no backslash, no leading slash. A directory entry ("word/", as docx-rs
// writes) is that path plus one trailing slash.
function canonicalPartName(name: string): boolean {
  const path = name.endsWith("/") ? name.slice(0, -1) : name;
  return !path.includes("\\") && path.split("/").every((s) => s !== "" && s !== "." && s !== "..");
}

// The archive must be structurally sound with canonical part names; every
// DOCX word/ or PPTX ppt/slides/ XML part must be well-formed UTF-8 XML, and
// one of them holds the text. Only those parts are inflated.
export function officeHasText(bytes: Uint8Array, extension: string, text: string): boolean {
  const prefix = extension === "docx" ? "word/" : "ppt/slides/";
  const archive = openZip(bytes);
  const bad = archive.names.find((name) => !canonicalPartName(name));
  check(bad === undefined, `${extension} export has a non-canonical part name ${String(bad)}`);
  const parts = archive.names
    .filter((name) => name.startsWith(prefix) && name.endsWith(".xml"))
    .map((name) => xmlText(decodeUtf8(archive.read(name))));
  return parts.some((part) => part.includes(text));
}

function field(record: Record<string, unknown>, key: string): unknown {
  check(Object.hasOwn(record, key), `response has no ${key}: ${JSON.stringify(record)}`);
  return record[key];
}

async function create(client: Client, workspace: string, statePath: string): Promise<void> {
  const root = `/api/v1/workspaces/${workspace}`;
  const archive = writeZip([{ name: "설치 검증.md", data: encoder.encode(MARKDOWN) }]);
  const job = await client.api("/api/v1/import", {
    method: "POST",
    body: {
      workspaceId: workspace,
      source: "markdown-zip",
      zipBase64: Buffer.from(archive).toString("base64"),
    },
    expected: 201,
  });
  check(field(job, "status") === "completed", `import job status ${String(job.status)}`);
  const created = field(job, "createdDocumentIds");
  check(
    Array.isArray(created) && created.length === 1,
    `import created ${JSON.stringify(created)}`,
  );
  const document: unknown = created[0];
  const path = `${root}/documents/${String(document)}`;
  let body = field(await client.api(path + "/body"), "contentJson");
  const encoded = dumpJson(body, { ascii: false });
  for (const token of ["한글 본문", "🎉", '"table"', '"bold"', '"link"']) {
    check(encoded.includes(token), `imported body lacks ${token}`);
  }
  // Edit the imported live room through the normal Markdown write path.
  await client.json(path + "/body", {
    method: "PUT",
    body: { contentMd: MARKDOWN + `\n${EDIT}\n` },
  });
  body = field(await client.api(path + "/body"), "contentJson");
  check(dumpJson(body, { ascii: false }).includes(EDIT), "the Markdown edit is not in the body");
  for (const [extension, mime] of EXPORTS) {
    const { body: payload, headers } = await client.request(`${path}/${extension}`);
    const type = headers.get("content-type");
    check(type?.startsWith(mime) === true, `${extension}: Content-Type ${String(type)}`);
    const cache = headers.get("cache-control");
    check(cache === "private, no-store", `${extension}: Cache-Control ${String(cache)}`);
    check(payload.length > 0, `${extension}: empty export`);
    if (extension === "md") check(decodeUtf8(payload).includes(EDIT), "md export lacks the edit");
    else if (extension === "pdf") check(isPdf(payload), "pdf export is not a complete PDF");
    else check(officeHasText(payload, extension, EDIT), `${extension} export lacks the edit`);
    await client.request(`${path}/${extension}`, { authenticated: false, expected: 401 });
  }
  const share = await client.api(path + "/share-links", {
    method: "POST",
    body: {},
    expected: 201,
  });
  const url = String(field(share, "url"));
  check(url.includes("/"), `share url ${url} has no token`);
  const token = url.slice(url.lastIndexOf("/") + 1);
  const shared = await client.request(`/api/v1/share/${token}/pdf`, { authenticated: false });
  check(startsWith(shared.body, "%PDF-"), "the shared PDF is not a PDF");
  await client.json(`${root}/share-links/${String(field(share, "id"))}`, { method: "DELETE" });
  await client.request(`/api/v1/share/${token}/pdf`, { authenticated: false, expected: 404 });
  writeFileSync(statePath, dumpJson({ document, body, job: field(job, "id") }));
  console.log(
    "installed Rust import, edit, MD/PDF/DOCX/PPTX exports, public PDF and revocation: ok",
  );
}

async function restart(client: Client, workspace: string, statePath: string): Promise<void> {
  const state: unknown = JSON.parse(readFileSync(statePath, "utf8"));
  check(isRecord(state), `${statePath} is not a JSON object`);
  const root = `/api/v1/workspaces/${workspace}`;
  const body = field(
    await client.api(`${root}/documents/${String(field(state, "document"))}/body`),
    "contentJson",
  );
  check(jsonEqual(body, field(state, "body")), "the document body changed across the restart");
  const job = await client.api(
    `/api/v1/import/${String(field(state, "job"))}?workspaceId=${workspace}`,
  );
  check(field(job, "status") === "completed", `import job status ${String(job.status)}`);
  console.log("imported and edited document plus completed import job survive restart: ok");
}

export async function main(argv: string[]): Promise<void> {
  check(
    argv.length === 5,
    "usage: smoke-documents.ts BASE_URL WORKSPACE_ID COOKIE_JAR STATE_FILE create|restart",
  );
  const [base = "", workspace = "", cookieFile = "", statePath = "", phase] = argv;
  // The jar's cookies are sent explicitly for BASE_URL's host, so a dotless
  // host such as http://localhost:8080 keeps curl's host-only session cookie.
  const jar = parseCookieJar(readFileSync(cookieFile, "utf8"), cookieFile);
  const client = new Client(base, cookieHeader(jar, base, cookieFile));
  if (phase === "create") await create(client, workspace, statePath);
  else if (phase === "restart") await restart(client, workspace, statePath);
  else throw new ReleaseError("phase must be create or restart");
}

if (import.meta.main) {
  try {
    await main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(
      `smoke-documents: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  }
}
