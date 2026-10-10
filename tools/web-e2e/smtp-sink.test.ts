import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { decodedText } from "./smtp-sink.ts";

const SINK = path.join(import.meta.dirname, "smtp-sink.ts");

type Sink = {
  proc: Bun.Subprocess<"ignore", "ignore", "pipe">;
  port: number;
  capture: string;
  dir: string;
};

const started: Sink[] = [];

afterEach(async () => {
  for (const sink of started.splice(0)) {
    sink.proc.kill("SIGKILL");
    await sink.proc.exited;
    rmSync(sink.dir, { recursive: true, force: true });
  }
});

function size(file: string): number {
  try {
    return statSync(file).size;
  } catch {
    return 0;
  }
}

// Same argv and readiness rule as scripts/web-e2e-inner.sh: truncate the
// capture, start in the background, wait for a non-empty port file while the
// child is alive.
async function startSink(): Promise<Sink> {
  const dir = mkdtempSync(path.join(tmpdir(), "fvoci-smtp-sink-"));
  const capture = path.join(dir, "smtp.jsonl");
  const portFile = path.join(dir, "smtp.port");
  writeFileSync(capture, "");
  const proc = Bun.spawn([process.execPath, SINK, "--capture", capture, "--port-file", portFile], {
    stdin: "ignore",
    stdout: "ignore",
    stderr: "pipe",
  });
  const sink: Sink = { proc, port: 0, capture, dir };
  started.push(sink);
  const deadline = Date.now() + 10_000;
  while (size(portFile) === 0) {
    if (proc.exitCode !== null || proc.signalCode !== null) {
      throw new Error("smtp sink exited before becoming ready");
    }
    if (Date.now() >= deadline) {
      throw new Error("smtp sink did not write port file");
    }
    await Bun.sleep(10);
  }
  sink.port = Number(readFileSync(portFile, "utf8"));
  return sink;
}

type Client = {
  send(text: string | Buffer): void;
  reply(): Promise<string>;
  end(): void;
  closed: Promise<void>;
};

// A real SMTP client over a TCP socket: replies are read one CRLF line at a time.
async function connect(port: number): Promise<Client> {
  let buffered = "";
  const waiters: (() => void)[] = [];
  let resolveClosed = () => {};
  const closed = new Promise<void>((resolve) => {
    resolveClosed = resolve;
  });
  const wake = () => {
    for (const waiter of waiters.splice(0)) {
      waiter();
    }
  };
  let ended = false;
  let pending = Buffer.alloc(0);
  const pump = (socket: Bun.Socket) => {
    if (pending.length > 0) {
      pending = pending.subarray(Math.max(socket.write(pending), 0));
    }
  };
  const socket = await Bun.connect({
    hostname: "127.0.0.1",
    port,
    socket: {
      data(_socket, chunk) {
        buffered += chunk.toString("utf8");
        wake();
      },
      drain(socket) {
        pump(socket);
      },
      close() {
        ended = true;
        resolveClosed();
        wake();
      },
    },
  });
  return {
    send(text) {
      pending = Buffer.concat([
        pending,
        typeof text === "string" ? Buffer.from(text, "utf8") : text,
      ]);
      pump(socket);
    },
    end() {
      socket.end();
    },
    async reply() {
      for (;;) {
        const end = buffered.indexOf("\r\n");
        if (end !== -1) {
          const line = buffered.slice(0, end);
          buffered = buffered.slice(end + 2);
          return line;
        }
        if (ended) {
          throw new Error("connection closed before a reply");
        }
        await new Promise<void>((resolve) => waiters.push(resolve));
      }
    },
    closed,
  };
}

type Captured = { from: string; to: string; data: string; text: string; ts: number };

function captured(sink: Sink): Captured[] {
  // The e2e consumer (apps/web/e2e/helpers.ts capturedMails) reads it this way.
  return readFileSync(sink.capture, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as Captured);
}

async function exchange(client: Client, line: string): Promise<string> {
  client.send(`${line}\r\n`);
  return client.reply();
}

describe("smtp-sink CLI", () => {
  test("captures a lettre-shaped message and dies by SIGTERM", async () => {
    const sink = await startSink();
    const client = await connect(sink.port);
    expect(await client.reply()).toBe("220 fvoci-smtp-sink");
    expect(await exchange(client, "EHLO fvoci.test")).toBe("250 fvoci");
    expect(await exchange(client, "MAIL FROM:<noreply@example.com>")).toBe("250 ok");
    expect(await exchange(client, "RCPT TO:<first@example.com>")).toBe("250 ok");
    expect(await exchange(client, "RCPT TO: <second@example.com>")).toBe("250 ok");
    expect(await exchange(client, "DATA")).toBe("354 go");
    const body = Buffer.from(
      "안녕하세요\nhttps://fvoci.test/reset-password?token=abc_-1\n",
    ).toString("base64");
    client.send(
      [
        "From: noreply@example.com",
        "Subject: FVOCI",
        " =?utf-8?b?67mE67CA67KI7Zi4?= =?utf-8?b?IOyerOyEpOyglQ==?=",
        "Content-Type: text/plain; charset=utf-8",
        "Content-Transfer-Encoding: base64",
        "",
        body,
        "..leading dot",
        ".",
        "",
      ].join("\r\n"),
    );
    expect(await client.reply()).toBe("250 ok");
    expect(await exchange(client, "QUIT")).toBe("221 bye");
    await client.closed;

    const mails = captured(sink);
    expect(mails).toHaveLength(1);
    const mail = mails[0];
    expect(mail?.from).toBe("noreply@example.com");
    // The last RCPT TO wins, as in the Python original.
    expect(mail?.to).toBe("second@example.com");
    expect(mail?.data.endsWith(`${body}\n.leading dot`)).toBe(true);
    expect(mail?.text).toContain("Subject: FVOCI 비밀번호 재설정");
    expect(mail?.text).toContain("/reset-password?token=abc_-1");
    expect(typeof mail?.ts).toBe("number");
    const line = readFileSync(sink.capture, "utf8");
    expect(
      line.startsWith('{"from": "noreply@example.com", "to": "second@example.com", "data": '),
    ).toBe(true);
    expect(line.endsWith("}\n")).toBe(true);

    sink.proc.kill("SIGTERM");
    expect(await sink.proc.exited).toBe(143);
    expect(sink.proc.exitCode).toBeNull();
    expect(sink.proc.signalCode).toBe("SIGTERM");
    expect(await new Response(sink.proc.stderr).text()).toBe(
      `smtp sink listening on 127.0.0.1:${String(sink.port)}\n`,
    );
  });

  test("pipelined commands, RSET, NOOP, unknown and oversize commands", async () => {
    const sink = await startSink();
    const client = await connect(sink.port);
    expect(await client.reply()).toBe("220 fvoci-smtp-sink");
    expect(await exchange(client, "helo x")).toBe("250 fvoci");
    expect(await exchange(client, "MAIL FROM:<dropped@example.com>")).toBe("250 ok");
    expect(await exchange(client, "RSET")).toBe("250 ok");
    expect(await exchange(client, "noop")).toBe("250 ok");
    expect(await exchange(client, "VRFY someone")).toBe("250 ok");
    expect(await exchange(client, `X${"y".repeat(200_000)}`)).toBe("250 ok");
    client.send(
      "RCPT TO:<only@example.com>\r\nDATA\r\nSubject: plain\r\n\r\nline\n.\r\n" +
        "MAIL FROM:<next@example.com>\r\nRCPT TO:<b@example.com>\r\nDATA\r\n" +
        "Subject: =?utf-8?q?caf=C3=A9_ok?=\r\nContent-Type: text/plain; charset=utf-8\r\n" +
        "Content-Transfer-Encoding: quoted-printable\r\n\r\nsoft=\r\nbreak =EC=95=88\r\n.\r\nQUIT\r\n",
    );
    const replies = [];
    for (let i = 0; i < 8; i += 1) {
      replies.push(await client.reply());
    }
    expect(replies).toEqual([
      "250 ok",
      "354 go",
      "250 ok",
      "250 ok",
      "250 ok",
      "354 go",
      "250 ok",
      "221 bye",
    ]);
    await client.closed;
    const mails = captured(sink);
    expect(mails.map(({ from, to }) => [from, to])).toEqual([
      ["", "only@example.com"],
      ["next@example.com", "b@example.com"],
    ]);
    expect(mails[0]?.data).toBe("Subject: plain\n\nline");
    // postal-mime keeps the body's last line break (see the intent table).
    expect(mails[0]?.text).toBe("Subject: plain\n\nline\n");
    expect(mails[1]?.text).toBe("Subject: café ok\n\nsoftbreak 안\n");

    sink.proc.kill("SIGINT");
    expect(await sink.proc.exited).toBe(130);
    expect(sink.proc.signalCode).toBe("SIGINT");
  });

  test("an unterminated DATA stores nothing", async () => {
    const sink = await startSink();
    const client = await connect(sink.port);
    expect(await client.reply()).toBe("220 fvoci-smtp-sink");
    client.send("MAIL FROM:<a@example.com>\r\nRCPT TO:<b@example.com>\r\nDATA\r\npartial\r\n");
    expect(await client.reply()).toBe("250 ok");
    expect(await client.reply()).toBe("250 ok");
    expect(await client.reply()).toBe("354 go");
    client.end();
    await client.closed;
    const other = await connect(sink.port);
    expect(await other.reply()).toBe("220 fvoci-smtp-sink");
    expect(await exchange(other, "QUIT")).toBe("221 bye");
    await other.closed;
    expect(captured(sink)).toEqual([]);
  });

  test("missing or unknown arguments exit 2 before binding", async () => {
    for (const args of [
      [],
      ["--capture", "x"],
      ["--capture", "x", "--port-file", "y", "--extra"],
    ]) {
      const proc = Bun.spawn([process.execPath, SINK, ...args], {
        stdin: "ignore",
        stdout: "ignore",
        stderr: "pipe",
      });
      expect(await proc.exited).toBe(2);
      expect(await new Response(proc.stderr).text()).toContain("usage: smtp-sink.ts");
    }
  });
});

describe("decodedText", () => {
  // Well-formed vectors on which the replaced Python sink (email.policy.default)
  // agrees except for the body's kept last line break. Malformed input and
  // multipart joining differ; those rows are in the intent table.
  test("header and body decoding vectors", async () => {
    expect(await decodedText("")).toBe("Subject: \n\n");
    expect(await decodedText("Subject: a  b\n\nbody")).toBe("Subject: a  b\n\nbody\n");
    expect(await decodedText("Subject: =?utf-8?b?7JWI?= tail\n\nx")).toBe(
      "Subject: 안 tail\n\nx\n",
    );
    expect(
      await decodedText("Subject: =?utf-8?b?7JWI?=x=?utf-8?b?7JWI?= =?utf-8?b?7JWI?=\n\n"),
    ).toBe("Subject: 안x안안\n\n");
    expect(await decodedText("Content-Type: text/html\n\n<p>x</p>")).toBe("Subject: \n\n");
    expect(
      await decodedText(
        "Subject: =?iso-8859-1?q?caf=E9?=\nContent-Type: text/plain; charset=iso-8859-1\n" +
          "Content-Transfer-Encoding: quoted-printable\n\ncaf=E9",
      ),
    ).toBe("Subject: café\n\ncafé\n");
  });

  // Inputs the hand-written decoder got wrong (PR #400 review B2).
  test("plain body selection and charset parameters", async () => {
    const link = "https://example.invalid/reset-password?token=abc_-1";
    expect(
      await decodedText(
        "Subject: multipart\nContent-Type: multipart/alternative; boundary=x\n\n--x\n" +
          `Content-Type: text/plain; charset=utf-8\n\n${link}\n--x\n` +
          "Content-Type: text/html; charset=utf-8\n\n<p>html</p>\n--x--",
      ),
    ).toBe(`Subject: multipart\n\n${link}\n`);
    expect(
      await decodedText(
        "Subject: attachment\nContent-Type: text/plain; charset=utf-8\n" +
          `Content-Disposition: attachment\n\n${link}`,
      ),
    ).toBe("Subject: attachment\n\n");
    expect(
      await decodedText(
        "Subject: comment\nContent-Type: text/plain; charset=utf-8 (comment)\n" +
          "Content-Transfer-Encoding: base64\n\n7JWI",
      ),
    ).toBe("Subject: comment\n\n안");
  });

  test("the declared charset applies to the received bytes", async () => {
    const sink = await startSink();
    const client = await connect(sink.port);
    expect(await client.reply()).toBe("220 fvoci-smtp-sink");
    expect(await exchange(client, "DATA")).toBe("354 go");
    client.send(
      Buffer.concat([
        Buffer.from("Subject: latin\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\ncaf"),
        Buffer.from([0xe9]),
        Buffer.from("\r\n.\r\n"),
      ]),
    );
    expect(await client.reply()).toBe("250 ok");
    expect(await exchange(client, "QUIT")).toBe("221 bye");
    await client.closed;
    const [mail] = captured(sink);
    expect(mail?.data).toBe("Subject: latin\nContent-Type: text/plain; charset=iso-8859-1\n\ncaf�");
    expect(mail?.text).toBe("Subject: latin\n\ncafé\n");
  });

  test("a message the parser rejects drops the connection without a record", async () => {
    let nested = "";
    for (let depth = 0; depth < 300; depth += 1) {
      nested += `Content-Type: multipart/mixed; boundary=b${String(depth)}\r\n\r\n--b${String(depth)}\r\n`;
    }
    const sink = await startSink();
    const client = await connect(sink.port);
    expect(await client.reply()).toBe("220 fvoci-smtp-sink");
    client.send(`DATA\r\n${nested}\r\nx\r\n.\r\nNOOP\r\n`);
    expect(await client.reply()).toBe("354 go");
    await client.closed;
    const after = await client.reply().then(
      () => "replied",
      (err: unknown) => String(err),
    );
    expect(after).toContain("connection closed before a reply");
    expect(captured(sink)).toEqual([]);
  });
});
