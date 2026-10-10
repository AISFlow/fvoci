import { describe, expect, test } from "bun:test";
import { connect } from "node:net";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { decodedText } from "./smtp-sink.ts";

function session(port: number, lines: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const socket = connect(port, "127.0.0.1");
    let response = "";
    socket.on("data", (chunk) => {
      response += chunk.toString("utf8");
    });
    socket.on("error", reject);
    socket.on("connect", () => {
      const payload = lines.join("\r\n") + "\r\n";
      const middle = Math.max(1, Math.floor(payload.length / 2));
      socket.write(payload.slice(0, middle));
      setTimeout(() => socket.write(payload.slice(middle)), 20);
    });
    socket.on("close", () => resolve(response));
    setTimeout(() => {
      socket.end();
    }, 500);
  });
}

describe("smtp sink", () => {
  test("decoded text keeps subject and plain body and drops a secret html alternative", () => {
    const data = [
      "Subject: =?UTF-8?B?7ZWc6riA?=",
      "MIME-Version: 1.0",
      "Content-Type: multipart/alternative; boundary=bound",
      "",
      "--bound",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Transfer-Encoding: quoted-printable",
      "",
      "open /reset-password?token=FIXEDTOKEN",
      "--bound",
      "Content-Type: text/html",
      "",
      "<b>SECRET-HTML</b>",
      "--bound--",
      "",
    ].join("\r\n");
    const text = decodedText(data);
    expect(text.startsWith("Subject: 한글")).toBe(true);
    expect(text).toContain("/reset-password?token=FIXEDTOKEN");
    expect(text).not.toContain("SECRET-HTML");
  });

  test("capture keeps envelope and decoded token and stderr does not", async () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-smtp-"));
    const capture = join(directory, "smtp.jsonl");
    const portFile = join(directory, "port");
    const child = Bun.spawn(
      ["bun", join(import.meta.dir, "smtp-sink.ts"), "--capture", capture, "--port-file", portFile],
      {
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    const deadline = Date.now() + 2000;
    while (Date.now() < deadline) {
      try {
        if (readFileSync(portFile, "utf8").trim()) break;
      } catch {
        // The port file appears after bind.
      }
      await Bun.sleep(20);
    }
    const port = Number(readFileSync(portFile, "utf8"));
    const body = [
      "Subject: reset",
      "Content-Type: text/plain; charset=utf-8",
      "",
      "visit /magic-link?token=FIXEDTOKEN",
      ".",
    ].join("\r\n");
    const response = await session(port, [
      "EHLO fixture",
      "MAIL FROM:<noreply@example.com>",
      "RCPT TO:<person@example.com>",
      "DATA",
      body,
      "RSET",
      "MAIL FROM:<second@example.com>",
      "RCPT TO:<other@example.com>",
      "NOOP",
      "QUIT",
    ]);
    child.kill();
    await child.exited;
    const stderr = await new Response(child.stderr).text();
    expect(stderr).toContain(`smtp sink listening on 127.0.0.1:${port}`);
    expect(stderr).not.toContain("noreply@example.com");
    expect(stderr).not.toContain("FIXEDTOKEN");
    expect(response).toContain("220 fvoci-smtp-sink");
    expect(response).toContain("354 go");
    expect(response).toContain("221 bye");
    const [record] = readFileSync(capture, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as { from: string; to: string; text: string; data: string });
    expect(record?.from).toBe("noreply@example.com");
    expect(record?.to).toBe("person@example.com");
    expect(record?.text).toBe("Subject: reset\n\nvisit /magic-link?token=FIXEDTOKEN");
    expect(record?.data).toContain("FIXEDTOKEN");
    expect(readFileSync(capture, "utf8").trim().split("\n")).toHaveLength(1);
  });
});
