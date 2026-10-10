import { afterAll, describe, expect, test } from "bun:test";
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  asciiJson,
  browserProcesses,
  sessionProcesses,
  shellSplit,
  startTicks,
} from "../../e2e-native-ime/proc";

test("shellSplit follows POSIX shell quoting and refuses unbalanced input", () => {
  expect(shellSplit(`chrome --a='x y' "q\\"\\z" b\\ c '' \t`)).toEqual([
    "chrome",
    "--a=x y",
    'q"\\z',
    "b c",
    "",
  ]);
  expect(shellSplit("  ")).toEqual([]);
  expect(() => shellSplit("a 'b")).toThrow("No closing quotation");
  expect(() => shellSplit('a "b\\')).toThrow("No escaped character");
  expect(() => shellSplit("a\\")).toThrow("No escaped character");
});

test("startTicks is not shifted by a comm containing spaces or ') '", () => {
  const fields = Array.from({ length: 50 }, (_, i) => String(i + 3));
  expect(startTicks(`42 (a) b c) ${fields.join(" ")}\n`)).toBe("22");
  expect(() => startTicks("42 (x) S 1")).toThrow("malformed");
});

test("asciiJson escapes DEL and non-ASCII like Python json.dumps(ensure_ascii=True)", () => {
  expect(asciiJson({ v: "한\u007f~\n" })).toBe('{\n  "v": "\\ud55c\\u007f~\\n"\n}');
});

// The native IME lane, like these scans, exists only on Linux.
describe.skipIf(process.platform !== "linux")("/proc process identification", () => {
  const bash = realpathSync(Bun.which("bash") ?? "/bin/bash");
  const root = mkdtempSync(join(tmpdir(), "native-ime-proc-"));
  const children: ReturnType<typeof Bun.spawn>[] = [];
  afterAll(async () => {
    for (const child of children) child.kill("SIGKILL");
    await Promise.all(children.map((child) => child.exited));
    rmSync(root, { recursive: true, force: true });
  });
  // One process per case, blocked on its stdin pipe and with no grandchildren. Bun.spawn
  // returns after exec, so /proc already shows the final argv; `title` becomes a
  // single-element argv like a rewritten Chromium process title.
  function hold(argv: string[], env: Record<string, string>, title?: string) {
    const child = title
      ? Bun.spawn(["cat"], { argv0: title, env, stdin: "pipe" })
      : Bun.spawn(["bash", "-c", "read -r _", "_", ...argv], { env, stdin: "pipe" });
    children.push(child);
    return String(child.pid);
  }

  test("sessionProcesses identifies owned processes by their exact session marker", () => {
    const session = join(root, "session");
    const base = { PATH: process.env.PATH ?? "" };
    const owned = hold([], { ...base, FVOCI_NATIVE_IME_SESSION: session, DISPLAY: ":42" });
    const noDisplay = hold([], { ...base, FVOCI_NATIVE_IME_SESSION: session });
    hold([], { ...base, FVOCI_NATIVE_IME_SESSION: session + "-other" });
    const rows = sessionProcesses(session);
    expect(rows.map((r) => String(r.pid)).sort()).toEqual([owned, noDisplay].sort());
    const row = rows.find((r) => String(r.pid) === owned);
    expect(row).toMatchObject({ exe: bash, display: ":42", command: "bash -c read -r _ _ " });
    expect(row?.start_ticks).toMatch(/^\d+$/);
    expect(rows.find((r) => String(r.pid) === noDisplay)?.display).toBeNull();
  });

  test("browserProcesses matches the exact profile flag and skips --type= children", () => {
    const profile = join(root, "profile");
    const env = { PATH: process.env.PATH ?? "", DISPLAY: ":43" };
    const browser = hold(["--user-data-dir=" + profile], env);
    hold(["--user-data-dir=" + profile, "--type=renderer"], env);
    hold(["--user-data-dir=" + profile + "x"], env);
    hold(["--user-data-dir", profile], env);
    const titled = hold([], env, `chrome --user-data-dir=${profile} 'two words'`);
    hold([], env, `chrome --user-data-dir=${profile} --type=gpu-process`);
    hold([], env, "unrelated host title 'unbalanced");
    const rows = browserProcesses(profile);
    expect(rows.map((r) => r.pid).sort()).toEqual([browser, titled].sort());
    expect(rows.find((r) => r.pid === titled)).toMatchObject({
      args: ["chrome", "--user-data-dir=" + profile, "two words"],
      display: ":43",
    });
    expect(rows.find((r) => r.pid === browser)?.exe).toBe(bash);
  });

  test("an unparseable title that carries a profile flag aborts the scan", async () => {
    const profile = join(root, "profile-unbalanced");
    const bad = Bun.spawn(["cat"], {
      argv0: `chrome --user-data-dir=${profile} 'open`,
      stdin: "pipe",
    });
    try {
      expect(() => browserProcesses(profile)).toThrow("No closing quotation");
    } finally {
      bad.kill("SIGKILL");
      await bad.exited;
    }
  });
});
