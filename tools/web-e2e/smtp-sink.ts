// Plaintext SMTP sink for FVOCI mail tests. Binds 127.0.0.1:0, writes the port
// and appends captured messages as JSON lines. No AUTH/TLS — matches the product
// client. Do not log envelope secrets beyond the captured file the caller owns.
//
// Usage: bun tools/web-e2e/smtp-sink.ts --capture <jsonl> --port-file <file>
// Replaces scripts/smtp-sink.py; intended differences are in the commit message.
import { appendFileSync, writeFileSync } from "node:fs";
// node:util's TextDecoder type takes any WHATWG label; Bun's global type lists
// only Bun.Encoding. Both are the same runtime class.
import { parseArgs, TextDecoder } from "node:util";

const IDLE_TIMEOUT_SECONDS = 30;
const LF = 0x0a;
const utf8 = new TextDecoder("utf-8", { ignoreBOM: true });

type Session = {
  buf: Buffer;
  partial: Buffer[];
  inData: boolean;
  dataLines: string[];
  mailFrom: string;
  rcptTo: string;
  out: Buffer;
  closing: boolean;
};

// --- Mail decoding (single-part text/plain, as the product's lettre client sends) ---

// WHATWG maps these Python codec names to windows-1252; Python decodes them
// as the codec itself.
const PYTHON_ASCII = new Set(["ascii", "us-ascii", "646"]);
const PYTHON_LATIN1 = new Set(["latin-1", "latin1", "iso-8859-1", "iso8859-1", "l1", "cp819"]);

// Decodes with a WHATWG label (or Python's spelling with "_"); undefined for
// an unknown charset.
function decodeWith(bytes: Uint8Array, charset: string): string | undefined {
  const label = charset.trim().toLowerCase().replace(/_/g, "-");
  if (PYTHON_ASCII.has(label)) {
    return Buffer.from(bytes)
      .toString("latin1")
      .replace(/[\x80-\xff]/g, "\ufffd");
  }
  if (PYTHON_LATIN1.has(label)) {
    return Buffer.from(bytes).toString("latin1");
  }
  try {
    // Every WHATWG label; RangeError for an unknown one.
    return new TextDecoder(label, { ignoreBOM: true }).decode(bytes);
  } catch {
    return undefined;
  }
}

function decodeCharset(bytes: Uint8Array, charset: string): string {
  return decodeWith(bytes, charset) ?? utf8.decode(bytes);
}

// A body charset Python cannot look up raises in its connection handler: the
// connection closes with no reply and nothing is stored. Throwing keeps that.
function decodeBodyCharset(bytes: Uint8Array, charset: string): string {
  const text = decodeWith(bytes, charset);
  if (text === undefined) {
    throw new Error("unknown body charset");
  }
  return text;
}

const HEX = /^[0-9A-Fa-f]{2}$/;

// Quoted-printable body decoding with the rules of CPython binascii.a2b_qp,
// which Python's email package applies: "=" before CR/LF is a soft break,
// "==" yields "=", an invalid escape keeps "=", trailing whitespace is kept.
function quotedPrintableBytes(text: string): Buffer {
  const data = Buffer.from(text, "utf8");
  const out: number[] = [];
  let i = 0;
  while (i < data.length) {
    const byte = data[i] ?? 0;
    i += 1;
    if (byte !== 0x3d) {
      out.push(byte);
      continue;
    }
    if (i >= data.length) {
      break;
    }
    const next = data[i] ?? 0;
    const pair = data.subarray(i, i + 2).toString("latin1");
    if (next === 0x0a || next === 0x0d) {
      while (i < data.length && data[i] !== 0x0a) {
        i += 1;
      }
      if (i < data.length) {
        i += 1;
      }
    } else if (next === 0x3d) {
      out.push(0x3d);
      i += 1;
    } else if (HEX.test(pair)) {
      out.push(parseInt(pair, 16));
      i += 2;
    } else {
      out.push(0x3d);
    }
  }
  return Buffer.from(out);
}

// Python ends an encoded word at the first "?=" unless two hex digits follow, so
// text may start with "=" only as a "=XX" escape.
const EW_TEXT = String.raw`(?:(?:=[0-9A-Fa-f]{2}|[^?\s=])[^?\s]*)?`;
const EW = String.raw`=\?([^?\s]+)\?([bBqQ])\?(${EW_TEXT})\?=`;
const ENCODED_WORD = new RegExp(EW, "g");
const LEADING_ENCODED_WORD = new RegExp(`^${EW}`);
const TRAILING_ENCODED_WORD = new RegExp(`${EW}$`);

function decodeEncodedWord(charset: string, kind: string, text: string): string {
  const bytes =
    kind.toUpperCase() === "B"
      ? Buffer.from(text, "base64")
      : Buffer.from(
          text
            .replace(/_/g, " ")
            .replace(/=([0-9A-Fa-f]{2})/g, (_m, hex: string) =>
              String.fromCharCode(parseInt(hex, 16)),
            ),
          "latin1",
        );
  // Python decodes an unknown encoded-word charset as UTF-8 (with a defect).
  return decodeCharset(bytes, charset.split("*")[0] ?? charset);
}

// One whitespace-free token; encoded words may sit anywhere inside it.
function decodeToken(token: string): {
  text: string;
  startsEncoded: boolean;
  endsEncoded: boolean;
} {
  return {
    text: token.replace(ENCODED_WORD, (_whole, charset: string, kind: string, text: string) =>
      decodeEncodedWord(charset, kind, text),
    ),
    startsEncoded: LEADING_ENCODED_WORD.test(token),
    endsEncoded: TRAILING_ENCODED_WORD.test(token),
  };
}

// RFC 2047 unstructured value as Python's email header parser decodes it:
// whitespace between a token ending and one starting with an encoded word is dropped.
function decodeHeaderValue(value: string): string {
  const parts = value.split(/([ \t]+)/);
  const tokens = parts.map((part, index) => (index % 2 === 0 ? decodeToken(part) : undefined));
  let result = "";
  parts.forEach((part, index) => {
    const token = tokens[index];
    if (token) {
      result += token.text;
    } else if (!(tokens[index - 1]?.endsEncoded && tokens[index + 1]?.startsEncoded)) {
      result += part;
    }
  });
  return result;
}

function parseMessage(data: string): { headers: [string, string][]; body: string } {
  const lines = data.split("\n");
  const headers: [string, string][] = [];
  let index = 0;
  for (; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    if (line === "") {
      index += 1;
      break;
    }
    const last = headers.at(-1);
    if (/^[\t ]/.test(line) && last) {
      last[1] += line;
      continue;
    }
    const match = /^([\x21-\x39\x3b-\x7e]+):(.*)$/.exec(line);
    if (!match) {
      break;
    }
    headers.push([match[1] ?? "", (match[2] ?? "").replace(/^[ \t]+/, "")]);
  }
  return { headers, body: lines.slice(index).join("\n") };
}

/** Subject and text/plain content as a mail client shows them. */
export function decodedText(data: string): string {
  const { headers, body } = parseMessage(data);
  const header = (name: string) =>
    headers.find(([key]) => key.toLowerCase() === name)?.[1].replace(/\r/g, "");
  const subject = decodeHeaderValue(header("subject") ?? "");
  const contentType = (header("content-type") ?? "text/plain").split(";");
  const mime = (contentType[0] ?? "").trim().toLowerCase();
  let content = "";
  if (!mime.includes("/") || mime === "text/plain") {
    const charsetParam = contentType
      .slice(1)
      .map((param) => /^\s*charset\s*=\s*"?([^";\s]*)"?\s*$/i.exec(param)?.[1])
      .find((value) => value !== undefined);
    const charset = charsetParam ?? "ascii";
    const encoding = (header("content-transfer-encoding") ?? "").trim().toLowerCase();
    if (encoding === "base64") {
      content = decodeBodyCharset(Buffer.from(body, "base64"), charset);
    } else if (encoding === "quoted-printable") {
      content = decodeBodyCharset(quotedPrintableBytes(body), charset);
    } else {
      // Identity transfer encodings keep the received text (see the intent table).
      decodeBodyCharset(new Uint8Array(), charset);
      content = body;
    }
  }
  return `Subject: ${subject}\n\n${content}`;
}

// --- SMTP session ---

function captureRecord(mailFrom: string, rcptTo: string, data: string): string {
  const ts = (performance.timeOrigin + performance.now()) / 1000;
  // Key order and separators of Python json.dumps(ensure_ascii=False).
  return `{"from": ${JSON.stringify(mailFrom)}, "to": ${JSON.stringify(rcptTo)}, "data": ${JSON.stringify(data)}, "text": ${JSON.stringify(decodedText(data))}, "ts": ${String(ts)}}\n`;
}

function envelopeAddress(line: string): string {
  return line
    .slice(line.indexOf(":") + 1)
    .trim()
    .replace(/^[<>]+|[<>]+$/g, "");
}

function flush(socket: Bun.Socket<Session>): void {
  const session = socket.data;
  if (session.out.length > 0) {
    const written = socket.write(session.out);
    session.out = session.out.subarray(Math.max(written, 0));
  }
  if (session.closing && session.out.length === 0) {
    socket.end();
  }
}

function reply(socket: Bun.Socket<Session>, line: string): void {
  const session = socket.data;
  session.out = Buffer.concat([session.out, Buffer.from(`${line}\r\n`, "utf8")]);
  flush(socket);
}

function handleLine(socket: Bun.Socket<Session>, capturePath: string, line: string): void {
  const session = socket.data;
  if (session.inData) {
    if (line === ".") {
      session.inData = false;
      const data = session.dataLines.join("\n");
      session.dataLines = [];
      appendFileSync(capturePath, captureRecord(session.mailFrom, session.rcptTo, data), "utf8");
      reply(socket, "250 ok");
      return;
    }
    session.dataLines.push(line.startsWith(".") ? line.slice(1) : line);
    return;
  }
  const upper = line.toUpperCase();
  if (upper.startsWith("EHLO") || upper.startsWith("HELO")) {
    reply(socket, "250 fvoci");
  } else if (upper.startsWith("MAIL FROM:")) {
    session.mailFrom = envelopeAddress(line);
    reply(socket, "250 ok");
  } else if (upper.startsWith("RCPT TO:")) {
    session.rcptTo = envelopeAddress(line);
    reply(socket, "250 ok");
  } else if (upper === "DATA") {
    session.inData = true;
    reply(socket, "354 go");
  } else if (upper === "QUIT") {
    reply(socket, "221 bye");
    session.closing = true;
    flush(socket);
  } else if (upper === "RSET") {
    session.mailFrom = "";
    session.rcptTo = "";
    reply(socket, "250 ok");
  } else {
    // NOOP and every unknown command.
    reply(socket, "250 ok");
  }
}

function onData(socket: Bun.Socket<Session>, capturePath: string, chunk: Buffer): void {
  const session = socket.data;
  // Join only once a line is complete, so a long line is not recopied per chunk.
  if (chunk.indexOf(LF) === -1) {
    session.partial.push(chunk);
    return;
  }
  session.buf = Buffer.concat([session.buf, ...session.partial.splice(0), chunk]);
  let newline = session.buf.indexOf(LF);
  while (newline !== -1 && !session.closing) {
    const raw = session.buf.subarray(0, newline);
    session.buf = session.buf.subarray(newline + 1);
    const line = utf8.decode(raw).replace(/\r+$/, "");
    try {
      handleLine(socket, capturePath, line);
    } catch {
      // A failed capture write drops the connection without a reply.
      session.closing = true;
      session.out = Buffer.alloc(0);
      socket.end();
      return;
    }
    newline = session.buf.indexOf(LF);
  }
}

function usage(message: string): never {
  process.stderr.write(
    `usage: smtp-sink.ts --capture CAPTURE --port-file PORT_FILE\nsmtp-sink.ts: error: ${message}\n`,
  );
  process.exit(2);
}

function main(argv: string[]): void {
  let values: { capture?: string; "port-file"?: string };
  try {
    ({ values } = parseArgs({
      args: argv,
      options: { capture: { type: "string" }, "port-file": { type: "string" } },
      strict: true,
      allowPositionals: false,
    }));
  } catch (err) {
    usage(err instanceof Error ? err.message : String(err));
  }
  const capturePath = values.capture;
  const portFile = values["port-file"];
  if (capturePath === undefined || portFile === undefined) {
    usage("the following arguments are required: --capture, --port-file");
  }
  const listener = Bun.listen<Session>({
    hostname: "127.0.0.1",
    port: 0,
    socket: {
      open(socket) {
        socket.data = {
          buf: Buffer.alloc(0),
          partial: [],
          inData: false,
          dataLines: [],
          mailFrom: "",
          rcptTo: "",
          out: Buffer.alloc(0),
          closing: false,
        };
        socket.timeout(IDLE_TIMEOUT_SECONDS);
        reply(socket, "220 fvoci-smtp-sink");
      },
      data(socket, chunk) {
        onData(socket, capturePath, chunk);
      },
      drain(socket) {
        flush(socket);
      },
      timeout(socket) {
        socket.end();
      },
      error(socket) {
        socket.end();
      },
    },
  });
  writeFileSync(portFile, String(listener.port), "utf8");
  process.stderr.write(`smtp sink listening on 127.0.0.1:${String(listener.port)}\n`);
}

if (import.meta.main) {
  main(process.argv.slice(2));
}
