import { once } from "node:events";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, test } from "bun:test";
import type { Subprocess } from "bun";

const root = join(import.meta.dir, "..");
const python = join(import.meta.dir, "smtp-sink.py");
const typescript = join(import.meta.dir, "smtp-sink.ts");

type Kind = "py" | "ts";
type Mail = { from: string; to: string; data: string; text: string; ts: number };
type Sink = {
  kind: Kind;
  proc: Subprocess;
  port: number;
  portFile: string;
  capture: string;
  stdout: () => Promise<string>;
  stderr: () => Promise<string>;
  stderrSoFar: () => string;
  stop: () => Promise<void>;
};

const running: Sink[] = [];
const dirs: string[] = [];

afterEach(async () => {
  await Promise.all(running.splice(0).map((sink) => sink.stop()));
  await Promise.all(dirs.splice(0).map((dir) => rm(dir, { recursive: true, force: true })));
});

function command(kind: Kind, args: string[]): string[] {
  // process.execPath is the bun binary running this file. "bun" on PATH may be
  // a wrapper that starts the real bun as a child; proc.kill() then leaves that
  // child alive and the stdout pipe open.
  return kind === "py" ? ["python3", python, ...args] : [process.execPath, typescript, ...args];
}

async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "fvoci-smtp-sink-"));
  dirs.push(dir);
  return dir;
}

async function cli(kind: Kind, args: string[]): Promise<{ stdout: string; stderr: string; exit: number }> {
  const proc = Bun.spawn(command(kind, args), {
    cwd: root,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exit] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { stdout, stderr, exit };
}

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

function connect(port: number): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "127.0.0.1", port }, () => resolve(socket));
    socket.setNoDelay(true);
    socket.on("error", reject);
  });
}

async function exchange(port: number, payload: string): Promise<string[]> {
  const socket = await connect(port);
  const lines = new LineBuffer(socket);
  socket.end(payload);
  const replies: string[] = [];
  try {
    for (;;) {
      const line = await lines.read();
      if (line === null) break;
      replies.push(line);
    }
    return replies;
  } finally {
    socket.destroy();
  }
}

async function lockstep(port: number, steps: { send: string; reply: boolean }[]): Promise<string[]> {
  const socket = await connect(port);
  const lines = new LineBuffer(socket);
  const greeting = await lines.read();
  if (greeting === null) throw new Error("missing greeting");
  const replies = [greeting];
  for (const step of steps) {
    socket.write(step.send);
    if (!step.reply) continue;
    const line = await lines.read();
    if (line === null) break;
    replies.push(line);
  }
  socket.end();
  if (!socket.closed) await once(socket, "close");
  return replies;
}

function contract(mail: Mail): { from: string; to: string; data: string; text: string } {
  return { from: mail.from, to: mail.to, data: mail.data, text: mail.text };
}

function captured(path: string): Mail[] {
  let raw = "";
  try {
    raw = readFileSync(path, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw err;
  }
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => {
      const value: unknown = JSON.parse(line);
      expect(value).toEqual({
        from: expect.any(String),
        to: expect.any(String),
        data: expect.any(String),
        text: expect.any(String),
        ts: expect.any(Number),
      });
      const mail = value as Mail;
      expect(Number.isFinite(mail.ts)).toBe(true);
      expect(Math.abs(mail.ts - Date.now() / 1000)).toBeLessThan(30);
      return mail;
    });
}

async function start(kind: Kind, dir: string, args?: { capture?: string; portFile?: string }): Promise<Sink> {
  const capture = args?.capture ?? join(dir, `${kind}.jsonl`);
  const portFile = args?.portFile ?? join(dir, `${kind}.port`);
  writeFileSync(capture, "");
  const proc = Bun.spawn(command(kind, ["--capture", capture, "--port-file", portFile]), {
    cwd: root,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const stdout = new Response(proc.stdout).text();
  const stderrReader = proc.stderr.getReader();
  let stderrText = "";
  const sink: Sink = {
    kind,
    proc,
    port: 0,
    portFile,
    capture,
    stdout: () => stdout,
    stderr: async () => stderrText,
    stderrSoFar: () => stderrText,
    stop: async () => {
      proc.kill();
      await proc.exited;
    },
  };
  running.push(sink);
  while (!stderrText.includes("\n")) {
    const { value, done } = await stderrReader.read();
    if (done) break;
    stderrText += Buffer.from(value).toString("utf8");
  }
  const drained = (async () => {
    while (true) {
      const { value, done } = await stderrReader.read();
      if (done) break;
      stderrText += Buffer.from(value).toString("utf8");
    }
  })();
  sink.stderr = async () => {
    await drained;
    return stderrText;
  };
  if (!stderrText.includes("listening")) {
    const exit = await proc.exited;
    throw new Error(`${kind} sink exited ${exit}: ${stderrText}`);
  }
  const filePort = readFileSync(portFile).toString("utf8");
  const announced = stderrText.match(/127\.0\.0\.1:(\d+)\n$/);
  expect(announced?.[1]).toBe(filePort);
  expect(filePort).toMatch(/^[0-9]+$/);
  sink.port = Number(filePort);
  expect(proc.exitCode).toBe(null);
  return sink;
}

async function pair(dir: string): Promise<{ py: Sink; ts: Sink }> {
  const [py, ts] = await Promise.all([start("py", dir), start("ts", dir)]);
  return { py, ts };
}

function quotedPrintable(text: string): string {
  let out = "";
  for (const byte of Buffer.from(text, "utf8")) {
    if (byte === 0x0a) out += "=0A";
    else if (byte >= 33 && byte <= 126 && byte !== 61) out += String.fromCharCode(byte);
    else out += `=${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

test("cli flags, help, and usage errors match the caller contract", async () => {
  const cases: { args: string[]; exit: number; help: boolean }[] = [
    { args: [], exit: 2, help: false },
    { args: ["--help"], exit: 0, help: true },
    { args: ["-h"], exit: 0, help: true },
    { args: ["--capture", "x"], exit: 2, help: false },
    { args: ["--port-file", "x"], exit: 2, help: false },
    { args: ["--capture"], exit: 2, help: false },
    { args: ["--port-file"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file"], exit: 2, help: false },
    { args: ["extra"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "extra"], exit: 2, help: false },
    { args: ["--unknown"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "--unknown"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "--foo", "--bar"], exit: 2, help: false },
    { args: ["--capture", "x", "--help"], exit: 0, help: true },
    { args: ["-h", "--capture", "x"], exit: 0, help: true },
    { args: ["--help", "--unknown"], exit: 0, help: true },
    { args: ["--unknown", "--help"], exit: 0, help: true },
    { args: ["--capture", "x", "--port-file", "y", "-h"], exit: 0, help: true },
    { args: ["--CAPTURE", "x", "--port-file", "y"], exit: 2, help: false },
    { args: ["--help=foo"], exit: 2, help: false },
    { args: ["--capture", "--port-file", "y"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "--capture"], exit: 2, help: false },
    { args: ["-"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "-"], exit: 2, help: false },
    { args: ["--capture", "x", "--port-file", "y", "--port"], exit: 2, help: false },
  ];
  for (const entry of cases) {
    const [py, ts] = await Promise.all([cli("py", entry.args), cli("ts", entry.args)]);
    expect(ts.exit).toBe(py.exit);
    expect(py.exit).toBe(entry.exit);
    expect(py.stdout === "" && ts.stdout === "").toBe(!entry.help);
    if (entry.help) {
      expect(py.stderr).toBe("");
      expect(ts.stderr).toBe("");
      expect(py.stdout).toContain("--capture");
      expect(py.stdout).toContain("--port-file");
      expect(ts.stdout).toContain("--capture");
      expect(ts.stdout).toContain("--port-file");
    } else {
      expect(py.stdout).toBe("");
      expect(ts.stdout).toBe("");
      expect(py.stderr.length).toBeGreaterThan(0);
      expect(ts.stderr.length).toBeGreaterThan(0);
    }
  }
});

test("a port file that cannot be created exits 1", async () => {
  const dir = await tempDir();
  const [py, ts] = await Promise.all([
    cli("py", ["--capture", join(dir, "mail.jsonl"), "--port-file", dir]),
    cli("ts", ["--capture", join(dir, "mail.jsonl"), "--port-file", dir]),
  ]);
  expect(py.exit).toBe(1);
  expect(ts.exit).toBe(py.exit);
  expect(py.stdout).toBe("");
  expect(ts.stdout).toBe("");
  expect(py.stderr.length).toBeGreaterThan(0);
  expect(ts.stderr.length).toBeGreaterThan(0);
});

test("caller start, lockstep SMTP, and capture fields match", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const subject = "워크스페이스 초대";
  const encoded = `=?utf-8?b?${Buffer.from(subject, "utf8").toString("base64")}?=`;
  const plain = "https://example.com/invite/TOKEN";
  const korean = "한글본문";
  const data = [
    `Subject: ${encoded}`,
    "Content-Type: text/plain; charset=utf-8",
    "Content-Transfer-Encoding: quoted-printable",
    "",
    plain,
    quotedPrintable(korean),
  ].join("\r\n");
  const steps = [
    { send: "EHLO test\r\n", reply: true },
    { send: "NOOP\r\n", reply: true },
    { send: "VRFY someone\r\n", reply: true },
    { send: "MAIL FROM:<noreply@example.com>\r\n", reply: true },
    { send: "RCPT TO:<invitee@example.com>\r\n", reply: true },
    { send: "DATA\r\n", reply: true },
    { send: `${data}\r\n.\r\n`, reply: true },
    { send: "RSET\r\n", reply: true },
    { send: "MAIL FROM:<noreply@example.com>\r\n", reply: true },
    { send: "RCPT TO:<second@example.com>\r\n", reply: true },
    { send: "DATA\r\n", reply: true },
    { send: "Subject: dots\r\n\r\n..hidden\r\n.\r\n", reply: true },
    { send: "QUIT\r\n", reply: true },
  ];
  const [pyReplies, tsReplies] = await Promise.all([lockstep(py.port, steps), lockstep(ts.port, steps)]);
  expect(pyReplies[0]).toStartWith("220 ");
  expect(tsReplies).toEqual(pyReplies);
  expect(pyReplies).toContain("354 go");
  expect(pyReplies).toContain("221 bye");
  const pyMail = captured(py.capture).map(contract);
  const tsMail = captured(ts.capture).map(contract);
  expect(tsMail).toEqual(pyMail);
  expect(pyMail).toHaveLength(2);
  expect(pyMail[0]).toMatchObject({
    from: "noreply@example.com",
    to: "invitee@example.com",
  });
  expect(pyMail[0]?.text).toContain(`Subject: ${subject}`);
  expect(pyMail[0]?.text).toContain(plain);
  expect(pyMail[0]?.text).toContain(korean);
  expect(pyMail[1]).toMatchObject({
    from: "noreply@example.com",
    to: "second@example.com",
    data: "Subject: dots\n\n.hidden",
  });
  expect(py.proc.exitCode).toBe(null);
  expect(ts.proc.exitCode).toBe(null);

  const reset = [
    "HELO other",
    "MAIL FROM:<noreply@example.com>",
    "RCPT TO:<Admin@Example.COM>",
    "DATA",
    "Subject: FVOCI 비밀번호 재설정",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "https://example.com/reset-password?token=abc_DEF-123",
    ".",
    "QUIT",
  ].join("\r\n") + "\r\n";
  const [pyReset, tsReset] = await Promise.all([exchange(py.port, reset), exchange(ts.port, reset)]);
  expect(tsReset).toEqual(pyReset);
  const pyAfter = captured(py.capture).map(contract);
  const tsAfter = captured(ts.capture).map(contract);
  expect(tsAfter).toEqual(pyAfter);
  expect(pyAfter[2]?.to).toBe("Admin@Example.COM");
  expect(pyAfter[2]?.text).toContain("Subject: FVOCI 비밀번호 재설정");
  expect(pyAfter[2]?.text).toContain("reset-password?token=abc_DEF-123");

  await Promise.all([py.stop(), ts.stop()]);
  expect(await py.stdout()).toBe("");
  expect(await ts.stdout()).toBe("");
  expect(await py.stderr()).toBe(`smtp sink listening on 127.0.0.1:${py.port}\n`);
  expect(await ts.stderr()).toBe(`smtp sink listening on 127.0.0.1:${ts.port}\n`);
  for (const sink of [py, ts]) {
    const log = await sink.stderr();
    expect(log).not.toContain("invitee@example.com");
    expect(log).not.toContain("noreply@example.com");
    expect(log).not.toContain("TOKEN");
  }
});

test("equals-form flags and a repeated --capture use the last path", async () => {
  const dir = await tempDir();
  const kinds: Kind[] = ["py", "ts"];
  const started: Sink[] = [];
  for (const kind of kinds) {
    const captureA = join(dir, `${kind}-a.jsonl`);
    const captureB = join(dir, `${kind}-b.jsonl`);
    const portFile = join(dir, `${kind}.port`);
    const proc = Bun.spawn(
      command(kind, ["--capture", captureA, "--port-file", portFile, "--capture", captureB]),
      { cwd: root, stdin: "ignore", stdout: "pipe", stderr: "pipe" },
    );
    const stdout = new Response(proc.stdout).text();
    const stderrReader = proc.stderr.getReader();
    let ready = "";
    const sink: Sink = {
      kind,
      proc,
      port: 0,
      portFile,
      capture: captureB,
      stdout: () => stdout,
      stderr: async () => ready,
      stderrSoFar: () => ready,
      stop: async () => {
        proc.kill();
        await proc.exited;
      },
    };
    running.push(sink);
    while (!ready.includes("\n")) {
      const { value, done } = await stderrReader.read();
      if (done) break;
      ready += Buffer.from(value).toString("utf8");
    }
    const announced = ready.match(/127\.0\.0\.1:(\d+)\n$/);
    const filePort = readFileSync(portFile).toString("utf8");
    expect(filePort).toBe(announced?.[1]);
    sink.port = Number(filePort);
    started.push(sink);
    expect(existsSync(captureA)).toBe(false);
  }
  await Promise.all(
    started.map((sink) =>
      exchange(
        sink.port,
        ["EHLO t", "MAIL FROM:<a@example.com>", "RCPT TO:<b@example.com>", "DATA", "Subject: later", "", "body", ".", "QUIT"].join(
          "\r\n",
        ) + "\r\n",
      ),
    ),
  );
  const records = started.map((sink) => captured(sink.capture).map(contract));
  expect(records[1]).toEqual(records[0]);
  expect(records[0]?.[0]?.to).toBe("b@example.com");
  expect(records[0]?.[0]?.text).toContain("Subject: later");
  for (const kind of kinds) {
    expect(existsSync(join(dir, `${kind}-a.jsonl`))).toBe(false);
  }

  const eqDir = await tempDir();
  const eq = await Promise.all(
    kinds.map(async (kind) => {
      const capture = join(eqDir, `${kind}.jsonl`);
      const portFile = join(eqDir, `${kind}.port`);
      writeFileSync(capture, "");
      const proc = Bun.spawn(command(kind, [`--capture=${capture}`, `--port-file=${portFile}`]), {
        cwd: root,
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
      });
      const stderrReader = proc.stderr.getReader();
      let ready = "";
      const sink: Sink = {
        kind,
        proc,
        port: 0,
        portFile,
        capture,
        stdout: async () => "",
        stderr: async () => ready,
        stderrSoFar: () => ready,
        stop: async () => {
          proc.kill();
          await proc.exited;
        },
      };
      running.push(sink);
      while (!ready.includes("\n")) {
        const { value, done } = await stderrReader.read();
        if (done) break;
        ready += Buffer.from(value).toString("utf8");
      }
      sink.port = Number(readFileSync(portFile).toString("utf8"));
      return sink;
    }),
  );
  const replies = await Promise.all(
    eq.map((sink) =>
      exchange(sink.port, ["EHLO t", "MAIL FROM:<>", "RCPT TO:<>", "DATA", "Subject: empty", "", ".", "QUIT"].join("\r\n") + "\r\n"),
    ),
  );
  expect(replies[1]).toEqual(replies[0]);
  const eqMail = eq.map((sink) => captured(sink.capture).map(contract));
  expect(eqMail[1]).toEqual(eqMail[0]);
  expect(eqMail[0]?.[0]).toMatchObject({ from: "", to: "" });
});

test("multipart, base64, and single-byte charsets match", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const messages = [
    [
      "Subject: A",
      "MIME-Version: 1.0",
      "Content-Type: multipart/alternative; boundary=bbb",
      "",
      "--bbb",
      "Content-Type: text/plain; charset=utf-8",
      "",
      "plain part",
      "--bbb",
      "Content-Type: text/html; charset=utf-8",
      "",
      "<b>html</b>",
      "--bbb--",
    ].join("\r\n"),
    [
      "Subject: M",
      "MIME-Version: 1.0",
      "Content-Type: multipart/mixed; boundary=bbb",
      "",
      "--bbb",
      "Content-Type: text/plain; charset=utf-8",
      "",
      "body",
      "--bbb",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Disposition: attachment; filename=a.txt",
      "",
      "secret-attachment",
      "--bbb--",
    ].join("\r\n"),
    [
      "Subject: H",
      "Content-Type: text/html; charset=utf-8",
      "",
      "<p>x</p>",
    ].join("\r\n"),
    [
      "Subject: B",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from("안녕\n", "utf8").toString("base64"),
    ].join("\r\n"),
    [
      "Subject: W",
      "Content-Type: text/plain; charset=windows-1252",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x80]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: L",
      "Content-Type: text/plain; charset=iso-8859-1",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x80, 0xe9]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: U",
      "Content-Type: text/plain; charset=us-ascii",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x80, 0xe9]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: D",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x80, 0xe9]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: S",
      "Content-Type: text/plain; charset*=utf-8''iso-8859-1",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x80, 0xe9]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: WU",
      "Content-Type: text/plain; charset=windows-1252",
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from([0x81, 0x80]).toString("base64"),
    ].join("\r\n"),
    [
      "Subject: =?utf-8?q?hello?= =?utf-8?q?world?=",
      "Content-Type: text/plain; charset=us-ascii",
      "",
      "folded",
    ].join("\r\n"),
    ["Subject: =?iso-8859-1?q?caf=E9?=", "", "ascii-body"].join("\r\n"),
  ];
  for (const message of messages) {
    const payload = ["EHLO t", "MAIL FROM:<a@example.com>", "RCPT TO:<b@example.com>", "DATA", message, ".", "QUIT"].join(
      "\r\n",
    ) + "\r\n";
    const [pyReplies, tsReplies] = await Promise.all([exchange(py.port, payload), exchange(ts.port, payload)]);
    expect(tsReplies).toEqual(pyReplies);
  }
  expect(captured(ts.capture).map(contract)).toEqual(captured(py.capture).map(contract));
  const mails = captured(py.capture);
  expect(mails[0]?.text).toBe("Subject: A\n\nplain part");
  expect(mails[1]?.text).toBe("Subject: M\n\nbody");
  expect(mails[1]?.text).not.toContain("secret-attachment");
  expect(mails[2]?.text).toBe("Subject: H\n\n");
  expect(mails[3]?.text).toContain("안녕");
  expect(mails[4]?.text).toContain("€");
  expect(mails[5]?.text).toBe("Subject: L\n\n\u0080é");
  expect(mails[6]?.text).toBe("Subject: U\n\n\uFFFD\uFFFD");
  expect(mails[7]?.text).toBe("Subject: D\n\n\uFFFD\uFFFD");
  expect(mails[8]?.text).toBe("Subject: S\n\n\u0080é");
  expect(mails[9]?.text).toBe("Subject: WU\n\n\uFFFD€");
  expect(mails[10]?.text).toContain("Subject: helloworld");
  expect(mails[11]?.text).toContain("Subject: café");
});

const pythonRejectedLabels = [
  "x-user-defined",
  "x-cp1252",
  "x-mac-roman",
  "unicode-1-1-utf-8",
  "unicode11utf8",
  "x-unicode20utf8",
];

function smtpData(message: string): string {
  return (
    ["EHLO t", "MAIL FROM:<a@example.com>", "RCPT TO:<b@example.com>", "DATA", message, ".", "QUIT"].join("\r\n") +
    "\r\n"
  );
}

test("labels python rejects are not acknowledged at top level or inside multipart", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  for (const label of pythonRejectedLabels) {
    const top = [
      `Subject: ${label}`,
      `Content-Type: text/plain; charset=${label}`,
      "",
      "hi",
    ].join("\r\n");
    const nested = [
      "Subject: part",
      "MIME-Version: 1.0",
      "Content-Type: multipart/mixed; boundary=bbb",
      "",
      "--bbb",
      `Content-Type: text/plain; charset=${label}`,
      "",
      "hi",
      "--bbb--",
    ].join("\r\n");
    for (const message of [top, nested]) {
      const [pyReplies, tsReplies] = await Promise.all([
        exchange(py.port, smtpData(message)),
        exchange(ts.port, smtpData(message)),
      ]);
      expect(tsReplies).toEqual(pyReplies);
      expect(acknowledged(pyReplies)).toBe(false);
      expect(acknowledged(tsReplies)).toBe(false);
    }
  }
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);
});

test("an unknown RFC 2231 charset* is not acknowledged", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const message = [
    "Subject: star",
    "Content-Type: text/plain; charset*=utf-8''not-a-charset",
    "",
    "hi",
  ].join("\r\n");
  const [pyReplies, tsReplies] = await Promise.all([
    exchange(py.port, smtpData(message)),
    exchange(ts.port, smtpData(message)),
  ]);
  expect(tsReplies).toEqual(pyReplies);
  expect(acknowledged(pyReplies)).toBe(false);
  expect(acknowledged(tsReplies)).toBe(false);
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);
});

test("a kelvin sign in a charset label is not a k", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const message = [
    "Subject: kelvin",
    `Content-Type: text/plain; charset=${"\u212a"}oi8-r`,
    "",
    "hi",
  ].join("\r\n");
  const [pyReplies, tsReplies] = await Promise.all([
    exchange(py.port, smtpData(message)),
    exchange(ts.port, smtpData(message)),
  ]);
  expect(tsReplies).toEqual(pyReplies);
  expect(acknowledged(pyReplies)).toBe(false);
  expect(acknowledged(tsReplies)).toBe(false);
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);
  const kept = ["Subject: kept", "Content-Type: text/plain; charset=KOI8-R", "", "hi"].join("\r\n");
  const [pyKept, tsKept] = await Promise.all([
    exchange(py.port, smtpData(kept)),
    exchange(ts.port, smtpData(kept)),
  ]);
  expect(tsKept).toEqual(pyKept);
  expect(acknowledged(pyKept)).toBe(true);
  expect(captured(ts.capture).map(contract)).toEqual(captured(py.capture).map(contract));
});

test("whitespace before charset* is still an extended parameter", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const unknown = [
    "Subject: gap",
    "Content-Type: text/plain; charset *=utf-8''not-a-charset",
    "",
    "hi",
  ].join("\r\n");
  const [pyReplies, tsReplies] = await Promise.all([
    exchange(py.port, smtpData(unknown)),
    exchange(ts.port, smtpData(unknown)),
  ]);
  expect(tsReplies).toEqual(pyReplies);
  expect(acknowledged(pyReplies)).toBe(false);
  expect(acknowledged(tsReplies)).toBe(false);
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);
  const known = [
    "Subject: gap-latin",
    "Content-Type: text/plain; charset *=utf-8''iso-8859-1",
    "Content-Transfer-Encoding: base64",
    "",
    Buffer.from([0x80, 0xe9]).toString("base64"),
  ].join("\r\n");
  const [pyKnown, tsKnown] = await Promise.all([
    exchange(py.port, smtpData(known)),
    exchange(ts.port, smtpData(known)),
  ]);
  expect(tsKnown).toEqual(pyKnown);
  expect(acknowledged(pyKnown)).toBe(true);
  expect(captured(ts.capture).map(contract)).toEqual(captured(py.capture).map(contract));
  expect(captured(py.capture)[0]?.text).toBe("Subject: gap-latin\n\n\u0080é");
});

test("leading BOMs match Python for utf-8 and utf-16", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const bodies: { subject: string; charset: string; bytes: number[]; text: string }[] = [
    { subject: "U8", charset: "utf-8", bytes: [0xef, 0xbb, 0xbf, 0x41], text: "Subject: U8\n\n\uFEFFA" },
    { subject: "U8A", charset: "u8", bytes: [0xef, 0xbb, 0xbf, 0x41], text: "Subject: U8A\n\n\uFEFFA" },
    { subject: "CP", charset: "cp65001", bytes: [0xef, 0xbb, 0xbf, 0x41], text: "Subject: CP\n\n\uFEFFA" },
    { subject: "LE", charset: "utf-16le", bytes: [0xff, 0xfe, 0x41, 0x00], text: "Subject: LE\n\n\uFEFFA" },
    { subject: "BE", charset: "utf-16be", bytes: [0xfe, 0xff, 0x00, 0x41], text: "Subject: BE\n\n\uFEFFA" },
    { subject: "BOM", charset: "utf-16", bytes: [0xfe, 0xff, 0x00, 0x41], text: "Subject: BOM\n\nA" },
  ];
  for (const body of bodies) {
    const message = [
      `Subject: ${body.subject}`,
      `Content-Type: text/plain; charset=${body.charset}`,
      "Content-Transfer-Encoding: base64",
      "",
      Buffer.from(body.bytes).toString("base64"),
    ].join("\r\n");
    const [pyReplies, tsReplies] = await Promise.all([
      exchange(py.port, smtpData(message)),
      exchange(ts.port, smtpData(message)),
    ]);
    expect(tsReplies).toEqual(pyReplies);
    expect(acknowledged(pyReplies)).toBe(true);
  }
  expect(captured(ts.capture).map(contract)).toEqual(captured(py.capture).map(contract));
  const texts = captured(py.capture).map((mail) => mail.text);
  expect(texts).toEqual(bodies.map((body) => body.text));
});

test("headers outside the supported grammar are not acknowledged", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const plain = (type: string, body = "hi", headers: string[] = []) =>
    ["Subject: g", ...headers, `Content-Type: ${type}`, "", body].join("\r\n");
  const messages = [
    plain("text/plain; charset (c)=bad"),
    plain("text/plain; (c) charset=utf-8"),
    plain(`text/plain; charset${"\u00a0"}=utf-8`),
    plain("text;"),
    plain("multipart/mixed; boundary=b "),
    plain("multipart/mixed; boundary=b(c)"),
    plain("multipart/mixed; boundary (c)=b"),
    plain("text/plain; charset=utf-8", "aGk=", ["Content-Transfer-Encoding: base64 (c)"]),
    plain("text/plain; charset=utf-8", "aGk=", ["Content-Transfer-Encoding: base64; x=y"]),
    plain("text/plain; charset=utf-8", "aGk=", [`Content-Transfer-Encoding: base64${"\u0085"}`]),
    plain("text/plain; charset=utf-8", "aGk=", ["Content-Transfer-Encoding: quoted-printable (c)"]),
    plain(`text/plain; charset=${"\u000b"}utf-8`),
    plain(`text/plain; charset=${"\u000c"}utf-8`),
    plain("text/plain; charset=utf-8", "aGk=x", ["Content-Transfer-Encoding: base64"]),
    plain("text/plain; charset=utf-8", "aGk*!!=x", ["Content-Transfer-Encoding: base64"]),
    plain("text/plain; charset=utf\t8"),
    plain(`text/plain; charset=${"\u0130"}SO-8859-1`),
    plain("text/plain; charset* =utf-8''utf-8"),
  ];
  for (const message of messages) {
    const tsBefore = captured(ts.capture).length;
    const pyBefore = captured(py.capture).length;
    const [pyReplies, tsReplies] = await Promise.all([
      exchange(py.port, smtpData(message)),
      exchange(ts.port, smtpData(message)),
    ]);
    expect(acknowledged(tsReplies)).toBe(false);
    expect(captured(ts.capture)).toHaveLength(tsBefore);
    if (!acknowledged(pyReplies)) {
      expect(tsReplies).toEqual(pyReplies);
      expect(captured(py.capture)).toHaveLength(pyBefore);
    }
  }
});

test("these messages get no 250 and an empty capture", async () => {
  // Python still accepts every message below, so this test does not call it.
  // Removing the Python sink must not drop these checks.
  // =?euc-kr?...?= is a real euc-kr word. charset=utf 8 is the token "utf" plus junk.
  // aGkx1 is stored by Python as the literal text aGkx1.
  // A leading BOM or a space before ":" makes Python keep the base64 text "aGk="
  // and still send 250, while recognizing the header would decode it to "hi".
  const dir = await tempDir();
  const ts = await start("ts", dir);
  const messages = [
    ["Subject: =?euc-kr?b?x9GxuQ==?=", "Content-Type: text/plain; charset=utf-8", "", "hi"].join("\r\n"),
    [
      "Subject: T",
      `${"\uFEFF"}Content-Type: text/plain; charset=utf-8`,
      "Content-Transfer-Encoding: base64",
      "",
      "aGk=",
    ].join("\r\n"),
    [
      "Subject: T",
      "Content-Type : text/plain; charset=utf-8",
      "Content-Transfer-Encoding: base64",
      "",
      "aGk=",
    ].join("\r\n"),
    [
      "Subject: T",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Transfer-Encoding: base64",
      "",
      "aGkx1",
    ].join("\r\n"),
    ["Subject: T", "Content-Type: text/plain; charset=utf 8", "", "A"].join("\r\n"),
  ];
  for (const message of messages) {
    const replies = await exchange(ts.port, smtpData(message));
    expect(acknowledged(replies)).toBe(false);
    expect(readFileSync(ts.capture, "utf8")).toBe("");
    expect(captured(ts.capture)).toEqual([]);
  }
});

test("charsets TextDecoder does not match are not acknowledged", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const labels = ["euc-kr", "iso-2022-jp", "gb2312", "gbk", "shift_jis"];
  for (const label of labels) {
    const message = [`Subject: ${label}`, `Content-Type: text/plain; charset=${label}`, "", "x"].join("\r\n");
    const [pyReplies, tsReplies] = await Promise.all([
      exchange(py.port, smtpData(message)),
      exchange(ts.port, smtpData(message)),
    ]);
    expect(acknowledged(pyReplies)).toBe(true);
    expect(acknowledged(tsReplies)).toBe(false);
    expect(tsReplies).not.toEqual(pyReplies);
  }
  expect(captured(ts.capture)).toEqual([]);
  expect(captured(py.capture)).toHaveLength(labels.length);
});

test("dropped DATA, unknown charset, and an unwritable capture are not acknowledged", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  const partial = ["EHLO t", "MAIL FROM:<a@example.com>", "RCPT TO:<b@example.com>", "DATA", "Subject: partial", "", "not finished"].join(
    "\r\n",
  ) + "\r\n";
  const [pyDrop, tsDrop] = await Promise.all([exchange(py.port, partial), exchange(ts.port, partial)]);
  expect(tsDrop).toEqual(pyDrop);
  expect(acknowledged(pyDrop)).toBe(false);
  expect(acknowledged(tsDrop)).toBe(false);
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);

  const bad = [
    "EHLO t",
    "MAIL FROM:<a@example.com>",
    "RCPT TO:<b@example.com>",
    "DATA",
    "Subject: bad",
    "Content-Type: text/plain; charset=not-a-charset",
    "",
    "BODYTOKEN",
    ".",
    "QUIT",
  ].join("\r\n") + "\r\n";
  const [pyBad, tsBad] = await Promise.all([exchange(py.port, bad), exchange(ts.port, bad)]);
  expect(tsBad).toEqual(pyBad);
  expect(pyBad).toContain("354 go");
  expect(acknowledged(pyBad)).toBe(false);
  expect(acknowledged(tsBad)).toBe(false);
  expect(captured(py.capture)).toEqual([]);
  expect(captured(ts.capture)).toEqual([]);
  expect(ts.stderrSoFar()).toBe(`smtp sink listening on 127.0.0.1:${ts.port}\n`);
  expect(ts.stderrSoFar()).not.toContain("BODYTOKEN");

  await Promise.all([py.stop(), ts.stop()]);
  const blocked = await tempDir();
  const sinks = await Promise.all(
    (["py", "ts"] as const).map(async (kind) => {
      const capture = join(blocked, kind);
      const portFile = join(blocked, `${kind}.port`);
      await mkdir(capture);
      const proc = Bun.spawn(command(kind, ["--capture", capture, "--port-file", portFile]), {
        cwd: root,
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
      });
      const stderrReader = proc.stderr.getReader();
      let ready = "";
      const sink: Sink = {
        kind,
        proc,
        port: 0,
        portFile,
        capture,
        stdout: async () => "",
        stderr: async () => ready,
        stderrSoFar: () => ready,
        stop: async () => {
          proc.kill();
          await proc.exited;
        },
      };
      running.push(sink);
      while (!ready.includes("\n")) {
        const { value, done } = await stderrReader.read();
        if (done) break;
        ready += Buffer.from(value).toString("utf8");
      }
      sink.port = Number(readFileSync(portFile).toString("utf8"));
      return sink;
    }),
  );
  const payload = ["EHLO t", "MAIL FROM:<a@example.com>", "RCPT TO:<b@example.com>", "DATA", "Subject: x", "", "y", ".", "QUIT"].join(
    "\r\n",
  ) + "\r\n";
  const blockedReplies = await Promise.all(sinks.map((sink) => exchange(sink.port, payload)));
  expect(blockedReplies[1]).toEqual(blockedReplies[0]);
  expect(blockedReplies[0]).toContain("354 go");
  expect(acknowledged(blockedReplies[0] ?? [])).toBe(false);
  expect(acknowledged(blockedReplies[1] ?? [])).toBe(false);
  expect(sinks[0]?.proc.exitCode).toBe(null);
  expect(sinks[1]?.proc.exitCode).toBe(null);
});

function acknowledged(replies: string[]): boolean {
  const data = replies.indexOf("354 go");
  if (data < 0) return false;
  return replies.slice(data + 1).some((line) => line.startsWith("250 "));
}

test("two clients can deliver without one blocking the other", async () => {
  const dir = await tempDir();
  const { py, ts } = await pair(dir);
  async function deliver(port: number, marker: string): Promise<void> {
    await exchange(
      port,
      ["EHLO t", "MAIL FROM:<a@example.com>", `RCPT TO:<${marker}@example.com>`, "DATA", `Subject: ${marker}`, "", marker, ".", "QUIT"].join(
        "\r\n",
      ) + "\r\n",
    );
  }
  await Promise.all([
    deliver(py.port, "one"),
    deliver(py.port, "two"),
    deliver(ts.port, "one"),
    deliver(ts.port, "two"),
  ]);
  const pyMail = captured(py.capture)
    .map(contract)
    .sort((a, b) => a.to.localeCompare(b.to));
  const tsMail = captured(ts.capture)
    .map(contract)
    .sort((a, b) => a.to.localeCompare(b.to));
  expect(tsMail).toEqual(pyMail);
  expect(pyMail.map((mail) => mail.to)).toEqual(["one@example.com", "two@example.com"]);
});

test("explicit -h/--help is help, and option abbreviations are rejected", async () => {
  const helpNextToMissingValue = await cli("ts", ["--capture", "--help"]);
  expect(helpNextToMissingValue.exit).toBe(0);
  expect(helpNextToMissingValue.stdout).toContain("--port-file");

  const abbreviated = await cli("ts", ["--cap", "x", "--port-file", "y"]);
  expect(abbreviated.exit).toBe(2);
  expect(abbreviated.stdout).toBe("");
});

test("an 8bit utf-8 body is the text a mail client shows", async () => {
  const dir = await tempDir();
  const sink = await start("ts", dir);
  await exchange(
    sink.port,
    [
      "EHLO t",
      "MAIL FROM:<a@example.com>",
      "RCPT TO:<b@example.com>",
      "DATA",
      "Subject: 안내",
      "Content-Type: text/plain; charset=utf-8",
      "",
      "본문",
      ".",
      "QUIT",
    ].join("\r\n") + "\r\n",
  );
  const [mail] = captured(sink.capture);
  expect(mail?.text).toBe("Subject: 안내\n\n본문");
  expect(mail?.data).toContain("본문");
});
