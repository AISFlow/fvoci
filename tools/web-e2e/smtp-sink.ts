// Plaintext SMTP sink. Binds 127.0.0.1:0, writes the port, and appends captured
// messages as JSON lines. No AUTH/TLS. Envelope values stay in the capture file.

import { createServer, type Socket } from "node:net";
import { appendFileSync, writeFileSync } from "node:fs";

function decodeQuotedPrintable(input: string): Buffer {
  const stripped = input.replace(/=\r?\n/g, "");
  const bytes: number[] = [];
  for (let index = 0; index < stripped.length; index += 1) {
    if (stripped[index] === "=" && /^[0-9A-Fa-f]{2}$/.test(stripped.slice(index + 1, index + 3))) {
      bytes.push(Number.parseInt(stripped.slice(index + 1, index + 3), 16));
      index += 2;
    } else bytes.push(stripped.charCodeAt(index));
  }
  return Buffer.from(bytes);
}

function decodeRfc2047(value: string): string {
  return value.replace(
    /=\?([^?]+)\?([BbQq])\?([^?]*)\?=/g,
    (_all, _charset: string, encoding: string, text: string) => {
      if (encoding.toUpperCase() === "B") return Buffer.from(text, "base64").toString("utf8");
      return decodeQuotedPrintable(text.replaceAll("_", " ")).toString("utf8");
    },
  );
}

function headerValue(headers: string, name: string): string {
  const lines = headers.split(/\r?\n/);
  const unfolded: string[] = [];
  for (const line of lines) {
    if (/^[ \t]/.test(line) && unfolded.length) unfolded[unfolded.length - 1] += ` ${line.trim()}`;
    else unfolded.push(line);
  }
  const prefix = `${name.toLowerCase()}:`;
  const found = unfolded.find((line) => line.toLowerCase().startsWith(prefix));
  return found ? decodeRfc2047(found.slice(prefix.length).trim()) : "";
}

function contentType(headers: string): { type: string; boundary: string } {
  const raw = headerValue(headers, "content-type");
  const [type = "text/plain", ...params] = raw.split(";").map((part) => part.trim());
  const boundary =
    params
      .find((part) => part.toLowerCase().startsWith("boundary="))
      ?.slice("boundary=".length)
      .replace(/^"|"$/g, "") ?? "";
  return { type: type.toLowerCase(), boundary };
}

function transferDecode(headers: string, body: string): string {
  const encoding = headerValue(headers, "content-transfer-encoding").toLowerCase();
  if (encoding === "base64") return Buffer.from(body.replace(/\s/g, ""), "base64").toString("utf8");
  if (encoding === "quoted-printable") return decodeQuotedPrintable(body).toString("utf8");
  return body.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

function plainBody(data: string): string {
  const normalized = data.replace(/\r\n/g, "\n");
  const splitAt = normalized.indexOf("\n\n");
  const headers = splitAt < 0 ? normalized : normalized.slice(0, splitAt);
  const body = splitAt < 0 ? "" : normalized.slice(splitAt + 2);
  const type = contentType(headers);
  if (type.type.startsWith("multipart/") && type.boundary) {
    const parts = body.split(`--${type.boundary}`).slice(1, -1);
    for (const part of parts) {
      const text = part.replace(/^\n/, "").replace(/\n$/, "");
      const partSplit = text.indexOf("\n\n");
      const partHeaders = partSplit < 0 ? text : text.slice(0, partSplit);
      if (
        contentType(partHeaders).type.startsWith("text/plain") ||
        headerValue(partHeaders, "content-type") === ""
      ) {
        const partBody = partSplit < 0 ? "" : text.slice(partSplit + 2);
        return transferDecode(partHeaders, partBody);
      }
    }
    return "";
  }
  return transferDecode(headers, body);
}

export function decodedText(data: string): string {
  const normalized = data.replace(/\r\n/g, "\n");
  const splitAt = normalized.indexOf("\n\n");
  const headers = splitAt < 0 ? normalized : normalized.slice(0, splitAt);
  return `Subject: ${headerValue(headers, "subject")}\n\n${plainBody(data)}`;
}

function sendLine(socket: Socket, line: string) {
  socket.write(`${line}\r\n`);
}

function stripAngles(value: string): string {
  return value.trim().replace(/^[<>]+|[<>]+$/g, "");
}

export function handleClient(socket: Socket, capturePath: string) {
  socket.setTimeout(30_000);
  let mailFrom = "";
  let rcptTo = "";
  let buffer = Buffer.alloc(0);
  let readingData = false;
  const dataLines: string[] = [];
  let closed = false;
  const finish = () => {
    if (closed) return;
    closed = true;
    socket.destroy();
  };
  const takeLine = (): string | undefined => {
    const newline = buffer.indexOf(0x0a);
    if (newline < 0) return undefined;
    const raw = buffer.subarray(0, newline).toString("utf8").replace(/\r$/, "");
    buffer = buffer.subarray(newline + 1);
    return raw;
  };
  const pump = () => {
    try {
      for (;;) {
        const line = takeLine();
        if (line === undefined) return;
        if (readingData) {
          if (line === ".") {
            readingData = false;
            writeRecord(capturePath, mailFrom, rcptTo, dataLines.join("\n"));
            dataLines.length = 0;
            sendLine(socket, "250 ok");
          } else dataLines.push(line.startsWith(".") ? line.slice(1) : line);
          continue;
        }
        const upper = line.toUpperCase();
        if (upper.startsWith("EHLO") || upper.startsWith("HELO")) sendLine(socket, "250 fvoci");
        else if (upper.startsWith("MAIL FROM:")) {
          mailFrom = stripAngles(line.slice(line.indexOf(":") + 1));
          sendLine(socket, "250 ok");
        } else if (upper.startsWith("RCPT TO:")) {
          rcptTo = stripAngles(line.slice(line.indexOf(":") + 1));
          sendLine(socket, "250 ok");
        } else if (upper === "DATA") {
          readingData = true;
          sendLine(socket, "354 go");
        } else if (upper === "QUIT") {
          sendLine(socket, "221 bye");
          finish();
          return;
        } else if (upper === "RSET") {
          mailFrom = "";
          rcptTo = "";
          sendLine(socket, "250 ok");
        } else sendLine(socket, "250 ok");
      }
    } catch {
      finish();
    }
  };
  sendLine(socket, "220 fvoci-smtp-sink");
  socket.on("data", (chunk: Buffer) => {
    buffer = Buffer.concat([buffer, chunk]);
    pump();
  });
  socket.on("timeout", finish);
  socket.on("error", finish);
}

function writeRecord(capturePath: string, mailFrom: string, rcptTo: string, data: string) {
  const record = {
    from: mailFrom,
    to: rcptTo,
    data,
    text: decodedText(data),
    ts: Date.now() / 1000,
  };
  appendFileSync(capturePath, `${JSON.stringify(record)}\n`);
}

if (import.meta.main) {
  const capture = flag("--capture");
  const portFile = flag("--port-file");
  if (!capture || !portFile) {
    console.error("usage: smtp-sink.ts --capture FILE --port-file FILE");
    process.exit(2);
  }
  const server = createServer((socket) => handleClient(socket, capture));
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    const port = address && typeof address === "object" ? address.port : 0;
    writeFileSync(portFile, String(port));
    console.error(`smtp sink listening on 127.0.0.1:${port}`);
  });
}

function flag(name: string): string | undefined {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}
