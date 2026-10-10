// Plaintext SMTP sink for FVOCI mail tests. Binds 127.0.0.1:0, writes the port
// and appends captured messages as JSON lines. No AUTH/TLS — matches the product
// client. Do not log envelope secrets beyond the captured file the caller owns.
//
// Usage: bun tools/web-e2e/smtp-sink.ts --capture <jsonl> --port-file <file>
// Replaces scripts/smtp-sink.py; intended differences are in the commit message.
import { appendFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import PostalMime from "postal-mime";

const IDLE_TIMEOUT_SECONDS = 30;
const LF = 0x0a;
const CR = 0x0d;
const LF_BYTE = Buffer.from([LF]);
const utf8 = new TextDecoder("utf-8", { ignoreBOM: true });

type Session = {
  buf: Buffer;
  partial: Buffer[];
  inData: boolean;
  dataLines: string[];
  dataBytes: Buffer[];
  mailFrom: string;
  rcptTo: string;
  out: Buffer;
  closing: boolean;
  pumping: boolean;
};

// postal-mime's own default; set here so the rejection boundary is the sink's.
const MAX_MIME_NESTING_DEPTH = 256;

/** Subject and plain-text body as a mail client shows them. postal-mime does the
 * header, MIME, transfer-encoding, charset and body selection; a parse error rejects.
 * The sink passes the received bytes, so a declared charset applies to them. */
export async function decodedText(message: string | Uint8Array): Promise<string> {
  const email = await PostalMime.parse(message, { maxNestingDepth: MAX_MIME_NESTING_DEPTH });
  return `Subject: ${email.subject ?? ""}\n\n${email.text ?? ""}`;
}

// --- SMTP session ---

async function captureRecord(
  mailFrom: string,
  rcptTo: string,
  data: string,
  bytes: Buffer,
): Promise<string> {
  const text = await decodedText(bytes);
  const ts = (performance.timeOrigin + performance.now()) / 1000;
  // Key order and separators of Python json.dumps(ensure_ascii=False).
  return `{"from": ${JSON.stringify(mailFrom)}, "to": ${JSON.stringify(rcptTo)}, "data": ${JSON.stringify(data)}, "text": ${JSON.stringify(text)}, "ts": ${String(ts)}}\n`;
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

// Returns a promise only for the end of DATA, whose record is decoded
// asynchronously; the caller awaits it before reading the next line.
function handleLine(
  socket: Bun.Socket<Session>,
  capturePath: string,
  line: string,
  bytes: Buffer,
): Promise<void> | undefined {
  const session = socket.data;
  if (session.inData) {
    if (line === ".") {
      session.inData = false;
      const data = session.dataLines.join("\n");
      const message = Buffer.concat(
        session.dataBytes.flatMap((part, index) => (index === 0 ? [part] : [LF_BYTE, part])),
      );
      session.dataLines = [];
      session.dataBytes = [];
      return captureRecord(session.mailFrom, session.rcptTo, data, message).then((record) => {
        appendFileSync(capturePath, record, "utf8");
        reply(socket, "250 ok");
      });
    }
    const stuffed = line.startsWith(".");
    session.dataLines.push(stuffed ? line.slice(1) : line);
    session.dataBytes.push(stuffed ? bytes.subarray(1) : bytes);
    return undefined;
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
  return undefined;
}

// Handles buffered lines strictly in order. One pump runs per session; data
// arriving while it awaits a DATA record is only buffered and read afterwards,
// so pipelined commands are answered after that message's 250.
async function pump(socket: Bun.Socket<Session>, capturePath: string): Promise<void> {
  const session = socket.data;
  session.pumping = true;
  try {
    let newline = session.buf.indexOf(LF);
    while (newline !== -1 && !session.closing) {
      let end = newline;
      while (end > 0 && session.buf[end - 1] === CR) {
        end -= 1;
      }
      // Trailing CRs stripped; the decoded line is what Python's sink compared.
      const raw = session.buf.subarray(0, end);
      session.buf = session.buf.subarray(newline + 1);
      const pending = handleLine(socket, capturePath, utf8.decode(raw), raw);
      if (pending) {
        await pending;
      }
      newline = session.buf.indexOf(LF);
    }
  } finally {
    session.pumping = false;
  }
}

// A parse or capture write failure drops the connection without a reply. The
// rejection handler runs as a microtask, before any further socket event.
function drop(socket: Bun.Socket<Session>): void {
  const session = socket.data;
  session.closing = true;
  session.out = Buffer.alloc(0);
  socket.end();
}

function onData(socket: Bun.Socket<Session>, capturePath: string, chunk: Buffer): void {
  const session = socket.data;
  // Join only once a line is complete, so a long line is not recopied per chunk.
  if (chunk.indexOf(LF) === -1) {
    session.partial.push(chunk);
    return;
  }
  session.buf = Buffer.concat([session.buf, ...session.partial.splice(0), chunk]);
  if (!session.pumping) {
    pump(socket, capturePath).catch(() => {
      drop(socket);
    });
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
          dataBytes: [],
          mailFrom: "",
          rcptTo: "",
          out: Buffer.alloc(0),
          closing: false,
          pumping: false,
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
