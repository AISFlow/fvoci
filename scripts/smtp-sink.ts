// Loopback plaintext SMTP sink for FVOCI mail tests.
//
// Callers (scripts/start-test-smtp.sh, scripts/web-e2e-inner.sh, and
// apps/web/e2e helpers) depend on this contract:
// - Flags --capture and --port-file. Usage errors exit 2. If the port file
//   cannot be written, exit 1. A running sink does not exit.
// - The port file is the decimal port of a 127.0.0.1 listener and nothing else.
// - The capture file is JSON lines {from, to, data, text, ts} that JSON.parse
//   accepts. `to` is the envelope recipient. `text` is the message as a mail
//   client shows it: "Subject: …\n\n" plus the decoded text/plain body
//   (RFC 2047 subject, quoted-printable or base64, charset).
// - 250 after DATA is sent only once that line is appended. A dropped client
//   or a failed write is not acknowledged. Envelope values are not written
//   anywhere except the capture file.

import { appendFileSync, writeFileSync } from "node:fs";
import net from "node:net";
import { parseArgs } from "node:util";

const HELP = `Usage: smtp-sink --capture <path> --port-file <path>

Plaintext SMTP sink for mail tests. Listens on 127.0.0.1 and an ephemeral port.
Writes that port to --port-file and appends one JSON object per message to --capture.

Options:
  -h, --help       Show this help
      --capture    JSONL capture file
      --port-file  File that receives the listening port
`;

const IDLE_MS = 30_000;

class UnknownCharsetError extends Error {
  constructor(charset: string) {
    super(`unknown charset ${charset}`);
    this.name = "UnknownCharsetError";
  }
}

type Headers = Map<string, string>;

type Mime = {
  headers: Headers;
  body: string;
  parts: Mime[];
};

class LineBuffer {
  private buf: Buffer = Buffer.alloc(0);
  private queue: string[] = [];
  private ended = false;
  private waiter: ((line: string | null) => void) | null = null;

  constructor(socket: net.Socket) {
    socket.on("data", (chunk: Buffer) => {
      this.buf = Buffer.concat([this.buf, chunk]);
      this.drain();
    });
    const finish = () => {
      this.ended = true;
      this.drain();
    };
    socket.on("end", finish);
    socket.on("close", finish);
    socket.on("error", finish);
  }

  private drain(): void {
    while (true) {
      const idx = this.buf.indexOf(0x0a);
      if (idx < 0) break;
      let raw: Buffer = this.buf.subarray(0, idx);
      this.buf = Buffer.from(this.buf.subarray(idx + 1));
      if (raw.length > 0 && raw[raw.length - 1] === 0x0d) raw = raw.subarray(0, raw.length - 1);
      this.queue.push(new TextDecoder("utf-8", { fatal: false }).decode(raw));
    }
    this.flush();
  }

  private flush(): void {
    if (!this.waiter) return;
    if (this.queue.length > 0) {
      const line = this.queue.shift() ?? null;
      const waiter = this.waiter;
      this.waiter = null;
      waiter(line);
      return;
    }
    if (this.ended) {
      const waiter = this.waiter;
      this.waiter = null;
      waiter(null);
    }
  }

  read(): Promise<string | null> {
    if (this.queue.length > 0) return Promise.resolve(this.queue.shift() ?? null);
    if (this.ended) return Promise.resolve(null);
    return new Promise((resolve) => {
      this.waiter = resolve;
    });
  }
}

function splitHeadBody(data: string): { head: string; body: string } {
  const blank = data.indexOf("\n\n");
  if (blank >= 0) return { head: data.slice(0, blank), body: data.slice(blank + 2) };
  const lines = data.split("\n");
  const head: string[] = [];
  let index = 0;
  for (; index < lines.length; index++) {
    const line = lines[index] ?? "";
    const continued = head.length > 0 && (line.startsWith(" ") || line.startsWith("\t"));
    if (continued || line.includes(":")) {
      head.push(line);
      continue;
    }
    break;
  }
  return { head: head.join("\n"), body: lines.slice(index).join("\n") };
}

function parseHeaders(block: string): Headers {
  const headers: Headers = new Map();
  if (block.length === 0) return headers;
  const logical: string[] = [];
  for (const line of block.split("\n")) {
    if (logical.length > 0 && (line.startsWith(" ") || line.startsWith("\t"))) {
      logical[logical.length - 1] += line;
    } else {
      logical.push(line);
    }
  }
  for (const line of logical) {
    const colon = line.indexOf(":");
    if (colon < 0) continue;
    const name = line.slice(0, colon).trim().toLowerCase();
    const value = line.slice(colon + 1).replace(/^[ \t]+/, "");
    if (!headers.has(name)) headers.set(name, value);
  }
  return headers;
}

function splitSemicolons(value: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quoted = false;
  for (const char of value) {
    if (char === '"') quoted = !quoted;
    if (char === ";" && !quoted) {
      parts.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  if (current.length > 0) parts.push(current);
  return parts;
}

const TOKEN = /^[!#$%&'+\-.0-9A-Z^_`a-z|~]$/;

type Parameter = {
  name: string;
  section: number;
  extended: boolean;
  charset: string;
  value: string;
};

function isWsp(char: string | undefined): boolean {
  return char === " " || char === "\t";
}

function parseParameter(piece: string): Parameter | null {
  let i = 0;
  while (isWsp(piece[i])) i += 1;
  const nameStart = i;
  while (TOKEN.test(piece[i] ?? "")) i += 1;
  if (i === nameStart) return null;
  const name = asciiFold(piece.slice(nameStart, i));
  if (name === null) return null;
  // Python's attribute token consumes trailing CFWS before the RFC 2231 '*'.
  while (isWsp(piece[i])) i += 1;
  let section = 0;
  let extended = false;
  if (piece[i] === "*") {
    i += 1;
    if ((piece[i] ?? "") >= "0" && (piece[i] ?? "") <= "9") {
      const sectionStart = i;
      while ((piece[i] ?? "") >= "0" && (piece[i] ?? "") <= "9") i += 1;
      section = Number(piece.slice(sectionStart, i));
      if (piece[i] === "*") {
        extended = true;
        i += 1;
      }
    } else {
      extended = true;
    }
    // `charset* =...` is not an extended parameter. `charset *=...` is.
    if (isWsp(piece[i])) return null;
  }
  if (piece[i] !== "=") return null;
  i += 1;
  while (isWsp(piece[i])) i += 1;
  let raw = piece.slice(i);
  const quoted = raw.startsWith('"') && raw.endsWith('"') && raw.length >= 2;
  if (quoted) raw = raw.slice(1, -1);
  else if (raw.trim().length === 0) return null;
  let charset = "us-ascii";
  let value = raw;
  if (extended && section === 0) {
    const first = raw.indexOf("'");
    const second = first < 0 ? -1 : raw.indexOf("'", first + 1);
    if (first >= 0 && second >= first + 1) {
      charset = asciiFold(raw.slice(0, first)) || "us-ascii";
      value = raw.slice(second + 1);
    }
  }
  return { name, section, extended, charset, value };
}

function percentDecode(value: string): Uint8Array {
  const bytes: number[] = [];
  for (let i = 0; i < value.length; i += 1) {
    const hex = value.slice(i + 1, i + 3);
    if (value[i] === "%" && /^[0-9A-Fa-f]{2}$/.test(hex)) {
      bytes.push(Number.parseInt(hex, 16));
      i += 2;
      continue;
    }
    bytes.push((value.charCodeAt(i) ?? 0) & 0xff);
  }
  return Uint8Array.from(bytes);
}

function contentParameters(pieces: string[]): Map<string, string> {
  const grouped = new Map<string, Parameter[]>();
  for (const piece of pieces) {
    const parameter = parseParameter(piece);
    if (!parameter) continue;
    const list = grouped.get(parameter.name) ?? [];
    list.push(parameter);
    grouped.set(parameter.name, list);
  }
  const params = new Map<string, string>();
  for (const [name, parts] of grouped) {
    let ordered = [...parts].sort((left, right) => left.section - right.section);
    const first = ordered[0];
    if (!first) continue;
    if (!first.extended && ordered.length > 1 && ordered[1]?.section === 0) ordered = ordered.slice(0, 1);
    const values: string[] = [];
    let expect = 0;
    for (const parameter of ordered) {
      if (parameter.section !== expect) {
        if (!parameter.extended) continue;
      }
      expect += 1;
      if (!parameter.extended) {
        values.push(parameter.value);
        continue;
      }
      let decoded: string;
      try {
        decoded = decodeCharset(percentDecode(parameter.value), first.charset);
      } catch (err) {
        if (!(err instanceof UnknownCharsetError)) throw err;
        decoded = decodeAscii(percentDecode(parameter.value));
      }
      values.push(decoded);
    }
    params.set(name, values.join(""));
  }
  return params;
}

function contentType(headers: Headers): { type: string; params: Map<string, string> } {
  const raw = headers.get("content-type") ?? "text/plain";
  const pieces = splitSemicolons(raw);
  const type = (pieces.shift() ?? "text/plain").trim().toLowerCase();
  return { type, params: contentParameters(pieces) };
}

function isAttachment(headers: Headers): boolean {
  const disposition = headers.get("content-disposition");
  if (!disposition) return false;
  return (disposition.split(";")[0] ?? "").trim().toLowerCase() === "attachment";
}

function parseMime(data: string): Mime {
  const { head, body } = splitHeadBody(data);
  const headers = parseHeaders(head);
  const { type, params } = contentType(headers);
  const mime: Mime = { headers, body, parts: [] };
  if (!type.startsWith("multipart/")) return mime;
  const boundary = params.get("boundary");
  if (!boundary) return mime;
  const marker = `--${boundary}`;
  const lines = body.split("\n");
  let current: string[] | null = null;
  const chunks: string[] = [];
  for (const line of lines) {
    if (line === marker || line === `${marker}--`) {
      if (current) chunks.push(current.join("\n"));
      current = line.endsWith("--") ? null : [];
      if (line === `${marker}--`) break;
      continue;
    }
    if (current) current.push(line);
  }
  if (current) chunks.push(current.join("\n"));
  mime.parts = chunks.map((chunk) => parseMime(chunk));
  return mime;
}

// Labels Python's codecs.lookup accepts and this decoder matches on every
// byte 0x00-0xFF (singly and as one buffer). TextDecoder is used only for the
// labels where that comparison matched. ASCII and ISO-8859-1 are decoded
// here because TextDecoder maps both through windows-1252.
const ASCII_LABELS = new Set([
  "646",
  "ansi_x3.4-1968",
  "ansi_x3.4_1968",
  "ansi_x3.4_1986",
  "ansi_x3_4_1968",
  "ascii",
  "cp367",
  "csascii",
  "ibm367",
  "iso646_us",
  "iso_646.irv_1991",
  "iso_ir_6",
  "us",
  "us-ascii",
  "us_ascii",
]);

const LATIN1_LABELS = new Set([
  "8859",
  "cp819",
  "csisolatin1",
  "ibm819",
  "iso-8859-1",
  "iso8859",
  "iso8859-1",
  "iso8859_1",
  "iso_8859_1",
  "iso_8859_1_1987",
  "iso_ir_100",
  "l1",
  "latin",
  "latin-1",
  "latin1",
  "latin_1",
]);

const UTF8_LABELS = new Set(["cp65001", "u8", "utf", "utf-8", "utf8", "utf8_ucs2", "utf8_ucs4", "utf_8"]);

const TEXT_DECODER_LABELS = new Set([
  "866",
  "arabic",
  "cp1256",
  "cp866",
  "csibm866",
  "csisolatin2",
  "csisolatin3",
  "csisolatin4",
  "csisolatin6",
  "csisolatinarabic",
  "csisolatincyrillic",
  "csisolatingreek",
  "csisolatinhebrew",
  "cskoi8r",
  "cyrillic",
  "elot_928",
  "greek",
  "greek8",
  "hebrew",
  "ibm866",
  "iso-8859-15",
  "iso-8859-2",
  "koi8-r",
  "koi8_r",
  "l2",
  "l3",
  "l4",
  "l6",
  "l9",
  "latin2",
  "latin3",
  "latin4",
  "latin6",
  "macintosh",
  "utf-16",
  "utf-16be",
  "utf-16le",
]);

// Python's cp125x leaves these bytes undefined (U+FFFD). TextDecoder emits a
// C1 control or a different character for the same byte.
const CORRECTED_SINGLE_BYTE = new Map<string, { decoder: string; undefinedBytes: ReadonlySet<number> }>([
  ["1250", { decoder: "cp1250", undefinedBytes: new Set([0x81, 0x83, 0x88, 0x90, 0x98]) }],
  ["1251", { decoder: "cp1251", undefinedBytes: new Set([0x98]) }],
  ["1252", { decoder: "windows-1252", undefinedBytes: new Set([0x81, 0x8d, 0x8f, 0x90, 0x9d]) }],
  ["cp1250", { decoder: "cp1250", undefinedBytes: new Set([0x81, 0x83, 0x88, 0x90, 0x98]) }],
  ["cp1251", { decoder: "cp1251", undefinedBytes: new Set([0x98]) }],
  ["cp1252", { decoder: "cp1252", undefinedBytes: new Set([0x81, 0x8d, 0x8f, 0x90, 0x9d]) }],
  ["cp1253", { decoder: "cp1253", undefinedBytes: new Set([0x81, 0x88, 0x8a, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x98, 0x9a, 0x9c, 0x9d, 0x9e, 0x9f]) }],
  ["cp1254", { decoder: "cp1254", undefinedBytes: new Set([0x81, 0x8d, 0x8e, 0x8f, 0x90, 0x9d, 0x9e]) }],
  ["cp1255", { decoder: "cp1255", undefinedBytes: new Set([0x81, 0x8a, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x9a, 0x9c, 0x9d, 0x9e, 0x9f, 0xca]) }],
  ["cp1257", { decoder: "cp1257", undefinedBytes: new Set([0x81, 0x83, 0x88, 0x8a, 0x8c, 0x90, 0x98, 0x9a, 0x9c, 0x9f]) }],
  ["cp1258", { decoder: "cp1258", undefinedBytes: new Set([0x81, 0x8a, 0x8d, 0x8e, 0x8f, 0x90, 0x9a, 0x9d, 0x9e]) }],
  ["windows-1252", { decoder: "windows-1252", undefinedBytes: new Set([0x81, 0x8d, 0x8f, 0x90, 0x9d]) }],
  ["windows_1250", { decoder: "cp1250", undefinedBytes: new Set([0x81, 0x83, 0x88, 0x90, 0x98]) }],
  ["windows_1251", { decoder: "cp1251", undefinedBytes: new Set([0x98]) }],
  ["windows_1252", { decoder: "windows-1252", undefinedBytes: new Set([0x81, 0x8d, 0x8f, 0x90, 0x9d]) }],
]);

function asciiFold(value: string): string | null {
  let out = "";
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i) ?? 0;
    if (code > 0x7f) return null;
    out += code >= 0x41 && code <= 0x5a ? String.fromCharCode(code + 0x20) : (value[i] ?? "");
  }
  return out;
}

function foldCharsetLabel(charset: string): string {
  let start = 0;
  let end = charset.length;
  while (start < end && (charset.charCodeAt(start) === 0x20 || charset.charCodeAt(start) === 0x09)) start += 1;
  while (end > start && (charset.charCodeAt(end - 1) === 0x20 || charset.charCodeAt(end - 1) === 0x09)) end -= 1;
  const label = asciiFold(charset.slice(start, end));
  if (label === null) throw new UnknownCharsetError(charset);
  return label;
}

function decodeUtf8(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { ignoreBOM: true }).decode(bytes);
}

function decodeUtf16(bytes: Uint8Array): string {
  // Python's utf-16 consumes either BOM and does not emit U+FEFF. TextDecoder's
  // utf-16 label only treats FF FE as a BOM, so a FE FF prefix was decoded as
  // little-endian data (U+FFFE plus a swapped character).
  if (bytes.length >= 2 && bytes[0] === 0xfe && bytes[1] === 0xff) {
    return new TextDecoder("utf-16be", { ignoreBOM: true }).decode(bytes.subarray(2));
  }
  if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe) {
    return new TextDecoder("utf-16le", { ignoreBOM: true }).decode(bytes.subarray(2));
  }
  return new TextDecoder("utf-16le", { ignoreBOM: true }).decode(bytes);
}

function decodeAscii(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += byte < 0x80 ? String.fromCharCode(byte) : "\uFFFD";
  return out;
}

function decodeLatin1(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += String.fromCharCode(byte);
  return out;
}

function decodeCorrected(decoder: string, bytes: Uint8Array, undefinedBytes: ReadonlySet<number>): string {
  const decoded = new TextDecoder(decoder).decode(bytes);
  const chars = [...decoded];
  if (chars.length !== bytes.length) throw new UnknownCharsetError(decoder);
  for (let i = 0; i < bytes.length; i += 1) {
    if (undefinedBytes.has(bytes[i] ?? 0)) chars[i] = "\uFFFD";
  }
  return chars.join("");
}

function decodeCharset(bytes: Uint8Array, charset: string): string {
  const label = foldCharsetLabel(charset);
  if (ASCII_LABELS.has(label)) return decodeAscii(bytes);
  if (LATIN1_LABELS.has(label)) return decodeLatin1(bytes);
  if (UTF8_LABELS.has(label)) return decodeUtf8(bytes);
  const corrected = CORRECTED_SINGLE_BYTE.get(label);
  if (corrected) return decodeCorrected(corrected.decoder, bytes, corrected.undefinedBytes);
  if (label === "utf-16") return decodeUtf16(bytes);
  if (label === "utf-16le" || label === "utf-16be") {
    return new TextDecoder(label, { ignoreBOM: true }).decode(bytes);
  }
  if (TEXT_DECODER_LABELS.has(label)) return new TextDecoder(label).decode(bytes);
  throw new UnknownCharsetError(charset);
}

function decodeQuotedPrintable(input: string): Uint8Array {
  const bytes: number[] = [];
  for (let i = 0; i < input.length; i++) {
    const char = input[i];
    if (char === "=") {
      const hex = input.slice(i + 1, i + 3);
      if (input[i + 1] === "\n") {
        i += 1;
        continue;
      }
      if (/^[0-9A-Fa-f]{2}$/.test(hex)) {
        bytes.push(Number.parseInt(hex, 16));
        i += 2;
        continue;
      }
    }
    bytes.push((char ?? "").charCodeAt(0) & 0xff);
  }
  return Uint8Array.from(bytes);
}

function decodeQ(text: string): Uint8Array {
  const bytes: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === "_") {
      bytes.push(0x20);
      continue;
    }
    if (char === "=") {
      const hex = text.slice(i + 1, i + 3);
      if (/^[0-9A-Fa-f]{2}$/.test(hex)) {
        bytes.push(Number.parseInt(hex, 16));
        i += 2;
        continue;
      }
    }
    bytes.push((char ?? "").charCodeAt(0) & 0xff);
  }
  return Uint8Array.from(bytes);
}

function decodeEncodedWords(value: string): string {
  const expression = /=\?([^?]+)\?([BbQq])\?([^?]*)\?=/g;
  const matches = [...value.matchAll(expression)];
  if (matches.length === 0) return value;
  let out = "";
  let cursor = 0;
  let previous = false;
  for (const match of matches) {
    const start = match.index ?? 0;
    const gap = value.slice(cursor, start);
    const rawCharset = match[1] ?? "";
    const charset = rawCharset.split("*")[0] ?? rawCharset;
    const bytes =
      (match[2] ?? "").toLowerCase() === "b"
        ? Buffer.from(match[3] ?? "", "base64")
        : decodeQ(match[3] ?? "");
    let decoded: string;
    try {
      decoded = decodeCharset(bytes, charset);
    } catch (err) {
      if (!(err instanceof UnknownCharsetError)) throw err;
      // Python keeps the message and shows one U+FFFD per undecodable byte.
      decoded = decodeAscii(bytes);
    }
    if (!(previous && /^[ \t]*$/.test(gap))) out += gap;
    out += decoded;
    previous = true;
    cursor = start + match[0].length;
  }
  return out + value.slice(cursor);
}

function transferBytes(body: string, headers: Headers): Uint8Array {
  const encoding = (headers.get("content-transfer-encoding") ?? "7bit").trim().toLowerCase();
  if (encoding === "base64") return Buffer.from(body.replace(/\s+/g, ""), "base64");
  if (encoding === "quoted-printable") return decodeQuotedPrintable(body);
  return Buffer.from(body, "utf8");
}

function findPlain(mime: Mime): Mime | null {
  if (isAttachment(mime.headers)) return null;
  const { type } = contentType(mime.headers);
  if (type === "text/plain") return mime;
  if (!type.startsWith("multipart/")) return null;
  for (const part of mime.parts) {
    const found = findPlain(part);
    if (found) return found;
  }
  return null;
}

function mailText(data: string): string {
  const mime = parseMime(data);
  const subject = decodeEncodedWords(mime.headers.get("subject") ?? "");
  const plain = findPlain(mime);
  let content = "";
  if (plain) {
    const { params } = contentType(plain.headers);
    content = decodeCharset(transferBytes(plain.body, plain.headers), params.get("charset") ?? "us-ascii");
  }
  return `Subject: ${subject}\n\n${content}`;
}

function envelopeAddress(line: string): string {
  const colon = line.indexOf(":");
  const raw = (colon < 0 ? "" : line.slice(colon + 1)).trim();
  const start = raw.indexOf("<");
  const end = raw.indexOf(">", start + 1);
  if (start >= 0 && end > start) return raw.slice(start + 1, end);
  return raw;
}

function send(socket: net.Socket, line: string): Promise<void> {
  return new Promise((resolve, reject) => {
    socket.write(`${line}\r\n`, "utf8", (err) => (err ? reject(err) : resolve()));
  });
}

async function handleClient(socket: net.Socket, capturePath: string): Promise<void> {
  socket.setNoDelay(true);
  socket.setTimeout(IDLE_MS);
  socket.on("timeout", () => socket.destroy());
  const lines = new LineBuffer(socket);
  let mailFrom = "";
  let rcptTo = "";
  try {
    await send(socket, "220 fvoci-smtp-sink");
    for (;;) {
      const line = await lines.read();
      if (line === null) return;
      const upper = line.toUpperCase();
      if (upper.startsWith("EHLO") || upper.startsWith("HELO")) {
        await send(socket, "250 fvoci");
      } else if (upper.startsWith("MAIL FROM:")) {
        mailFrom = envelopeAddress(line);
        await send(socket, "250 ok");
      } else if (upper.startsWith("RCPT TO:")) {
        rcptTo = envelopeAddress(line);
        await send(socket, "250 ok");
      } else if (upper === "DATA") {
        await send(socket, "354 go");
        const dataLines: string[] = [];
        for (;;) {
          const bodyLine = await lines.read();
          if (bodyLine === null) return;
          if (bodyLine === ".") break;
          dataLines.push(bodyLine.startsWith(".") ? bodyLine.slice(1) : bodyLine);
        }
        const data = dataLines.join("\n");
        const record = {
          from: mailFrom,
          to: rcptTo,
          data,
          text: mailText(data),
          ts: Date.now() / 1000,
        };
        appendFileSync(capturePath, `${JSON.stringify(record)}\n`, "utf8");
        await send(socket, "250 ok");
      } else if (upper === "QUIT") {
        await send(socket, "221 bye");
        return;
      } else if (upper === "RSET") {
        mailFrom = "";
        rcptTo = "";
        await send(socket, "250 ok");
      } else {
        await send(socket, "250 ok");
      }
    }
  } catch (err) {
    if (err instanceof UnknownCharsetError) return;
    const code = (err as NodeJS.ErrnoException).code;
    if (code) return;
    throw err;
  } finally {
    socket.destroy();
  }
}

function fail(message: string): number {
  process.stderr.write(`smtp-sink: ${message}\n`);
  return 2;
}

function serve(capturePath: string, portFile: string): void {
  const server = net.createServer({ noDelay: true }, (socket) => {
    socket.on("error", () => socket.destroy());
    void handleClient(socket, capturePath).catch(() => socket.destroy());
  });
  server.on("error", () => {
    process.stderr.write("smtp-sink: could not listen on 127.0.0.1\n");
    process.exit(1);
  });
  server.listen({ host: "127.0.0.1", port: 0, backlog: 32 }, () => {
    const address = server.address();
    if (!address || typeof address === "string") {
      process.stderr.write("smtp-sink: could not listen on 127.0.0.1\n");
      process.exit(1);
    }
    try {
      writeFileSync(portFile, String(address.port));
    } catch {
      process.stderr.write("smtp-sink: could not write the port file\n");
      process.exit(1);
    }
    process.stderr.write(`smtp sink listening on 127.0.0.1:${address.port}\n`);
  });
}

function main(argv: string[]): number | void {
  if (argv.some((arg) => arg === "-h" || arg === "--help")) {
    process.stdout.write(HELP);
    return 0;
  }
  let capture: string | undefined;
  let portFile: string | undefined;
  try {
    const parsed = parseArgs({
      args: argv,
      options: {
        capture: { type: "string" },
        "port-file": { type: "string" },
        help: { type: "boolean", short: "h" },
      },
      strict: true,
      allowPositionals: false,
    });
    if (parsed.values.help === true) {
      process.stdout.write(HELP);
      return 0;
    }
    capture = parsed.values.capture;
    portFile = parsed.values["port-file"];
  } catch (err) {
    const message = err instanceof Error ? err.message : "invalid arguments";
    return fail(message);
  }
  if (typeof capture !== "string" || typeof portFile !== "string") {
    return fail("--capture and --port-file are required");
  }
  serve(capture, portFile);
}

if (import.meta.main) {
  const code = main(process.argv.slice(2));
  if (typeof code === "number") process.exit(code);
}
