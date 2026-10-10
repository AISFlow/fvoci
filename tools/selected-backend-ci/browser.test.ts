// Private runtime browser copy: prepareBrowser stages every installed
// component from the preparation owner's cache, admittedBrowser re-proves the
// copy, and the fixed 1000:1000 runtime actor reads it through setpriv.
// No Chromium, Bun or product process starts from the fixture assets.
import { spawnSync } from "bun";
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { admittedBrowser } from "./admission.ts";
import { gid, read, uid } from "./io.ts";
import { prepareBrowser } from "./runtime.ts";

const SHA = "a".repeat(40);
interface Fixture {
  base: string;
  cache: string;
  chrome: string;
  asset: string;
  output: string;
}
let f: Fixture;
const saved = { ...process.env };
const linux = process.platform === "linux";
const sudo = (...args: string[]) => {
  expect(spawnSync(["sudo", "-n", ...args], { stdout: "pipe", stderr: "pipe" }).exitCode).toBe(0);
};

beforeEach(() => {
  const base = mkdtempSync(join(tmpdir(), "fvoci-browser-assets-"));
  const cache = join(base, "cache"),
    directory = join(cache, "chromium-1243/chrome-linux64");
  mkdirSync(directory, { recursive: true });
  const chrome = join(directory, "chrome"),
    asset = join(directory, "deb.deps");
  writeFileSync(chrome, "NOT a browser executable", { mode: 0o755 });
  writeFileSync(asset, "qualified supplemental asset", { mode: 0o600 });
  for (const [component, name] of [
    ["chromium_headless_shell-1243", "headless_shell"],
    ["ffmpeg-1011", "ffmpeg-linux"],
  ] as const) {
    mkdirSync(join(cache, component));
    writeFileSync(join(cache, component, name), "NOT executable", { mode: 0o700 });
  }
  // The private runtime copy has its own /tmp parent, apart from the cache.
  const output = mkdtempSync(join(tmpdir(), "fvoci-browser-output-"));
  f = { base, cache, chrome, asset, output };
  process.env.GITHUB_SHA = SHA;
});
afterEach(() => {
  if (linux)
    for (const path of [f.base, f.output])
      spawnSync(["sudo", "-n", "chown", "-h", "-R", `${String(uid())}:${String(gid())}`, path]);
  rmSync(f.base, { recursive: true, force: true });
  rmSync(f.output, { recursive: true, force: true });
  for (const key of Object.keys(process.env))
    if (!(key in saved)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, saved);
});
const stage = (output = f.output) => prepareBrowser(output, f.chrome);

// Runs a script as a real actor with no supplementary groups, from a 0755
// copy of the runner modules and Bun (the checkout may be unreadable to it).
function runAs(
  actor: number,
  body: string,
  args: string[],
): { exitCode: number; stdout: string; stderr: string } {
  const probe = mkdtempSync(join(tmpdir(), "fvoci-browser-probe-"));
  try {
    chmodSync(probe, 0o755);
    const tools = join(probe, "tools/selected-backend-ci");
    mkdirSync(tools, { recursive: true });
    for (const name of readdirSync(import.meta.dir))
      if (name.endsWith(".ts") && !name.endsWith(".test.ts"))
        copyFileSync(join(import.meta.dir, name), join(tools, name));
    const bun = join(probe, "bun");
    copyFileSync(process.execPath, bun);
    chmodSync(bun, 0o755);
    const script = join(probe, "probe.ts");
    writeFileSync(script, body.replaceAll("@TOOLS@", tools));
    const result = spawnSync(
      [
        "sudo",
        "-n",
        "setpriv",
        `--reuid=${String(actor)}`,
        `--regid=${String(actor)}`,
        "--clear-groups",
        "env",
        "GITHUB_SHA=" + SHA,
        "PLAYWRIGHT_BROWSERS_PATH=" + join(f.output, "browser"),
        bun,
        script,
        ...args,
      ],
      { stdout: "pipe", stderr: "pipe" },
    );
    return {
      exitCode: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    };
  } finally {
    rmSync(probe, { recursive: true, force: true });
  }
}
// admittedBrowser + runtimeAccess as the fixed 1000:1000 runtime actor.
function runtimeProbe(): { exitCode: number; stdout: string; stderr: string } {
  sudo("chown", "-R", "1000:1000", f.output);
  try {
    return runAs(
      1000,
      `import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { admittedBrowser, browserInventory, runtimeAccess } from "@TOOLS@/admission.ts";
const current = admittedBrowser(process.argv[2] ?? "");
runtimeAccess([], { bun: { path: "/bin/true", sha256: "" }, chromium: { path: current, sha256: "" },
  chromium_directory_files: browserInventory(dirname(current)) as Record<string, string> });
process.stdout.write(JSON.stringify({ admitted: current, uid: process.getuid?.(), gid: process.getgid?.(),
  groups: process.getgroups?.(), supplemental: readFileSync(join(dirname(current), "deb.deps"), "utf8") }));
`,
      [f.output],
    );
  } finally {
    sudo("chown", "-R", `${String(uid())}:${String(gid())}`, f.output);
  }
}

describe.serial("private runtime browser copy", () => {
  test.skipIf(!linux)(
    "complete private copy keeps bytes and every installed component for the 1000 actor",
    () => {
      const current = stage();
      const receipt = read(join(f.output, "runtime-browser-stage.json")) as {
        files: Record<string, unknown>;
      };
      expect(Object.keys(receipt.files).sort()).toEqual([
        "chromium-1243",
        "chromium_headless_shell-1243",
        "ffmpeg-1011",
      ]);
      expect(readFileSync(join(dirname(current), "deb.deps"))).toEqual(readFileSync(f.asset));
      expect(statSync(f.asset).mode & 0o777).toBe(0o600);
      expect(statSync(f.cache).uid).toBe(uid());
      const runtime = runtimeProbe();
      expect(runtime.exitCode, runtime.stderr).toBe(0);
      expect(JSON.parse(runtime.stdout)).toEqual({
        admitted: current,
        uid: 1000,
        gid: 1000,
        groups: [],
        supplemental: "qualified supplemental asset",
      });
      expect(statSync(f.output).uid).toBe(uid());
      expect(statSync(f.output).mode & 0o777).toBe(0o700);
    },
  );
  test.skipIf(!linux)(
    "an owner-only asset in a 1001 cache: executable preflight passes, the 1001 copy serves 1000",
    () => {
      // The source cache belongs to a non-root 1001 preparation account. The
      // runtime actor can execute chrome but not read the owner-only asset,
      // so an executable-only preflight would pass and Chromium would fail.
      chmodSync(f.base, 0o755);
      sudo("chown", "-R", "1001:1001", f.cache);
      const preflight = spawnSync(
        ["sudo", "-n", "setpriv", "--reuid=1000", "--regid=1000", "--clear-groups", "cat", f.asset],
        { stdout: "pipe", stderr: "pipe" },
      );
      expect(preflight.exitCode).not.toBe(0);
      sudo("chown", "1001:1001", f.output);
      const staged = runAs(
        1001,
        `import process from "node:process";
import { prepareBrowser } from "@TOOLS@/runtime.ts";
prepareBrowser(process.argv[2] ?? "", process.argv[3] ?? "");
`,
        [f.output, f.chrome],
      );
      expect(staged.exitCode, staged.stderr).toBe(0);
      sudo("chown", "-R", `${String(uid())}:${String(gid())}`, f.output);
      const runtime = runtimeProbe();
      expect(runtime.exitCode, runtime.stderr).toBe(0);
      expect(JSON.parse(runtime.stdout)).toMatchObject({
        admitted: join(f.output, "browser/chromium-1243/chrome-linux64/chrome"),
        uid: 1000,
        gid: 1000,
        groups: [],
        supplemental: "qualified supplemental asset",
      });
      const source = spawnSync(["sudo", "-n", "stat", "-c", "%u:%g:%a", f.asset], {
        stdout: "pipe",
      });
      expect(source.stdout.toString().trim()).toBe("1001:1001:600");
    },
  );
  test("an unreadable source asset is refused before any copy", () => {
    chmodSync(f.asset, 0);
    try {
      expect(() => stage()).toThrow();
      expect(existsSync(join(f.output, "browser"))).toBe(false);
    } finally {
      chmodSync(f.asset, 0o600);
    }
  });
  test.skipIf(!linux)("foreign, root and root-group source assets are refused", () => {
    const foreign = Math.max(uid(), gid(), 1000) + 1;
    const owners: [number, number][] = [
      [foreign, foreign],
      [0, gid()],
      [uid(), 0],
      [uid(), foreign],
    ];
    owners.forEach(([owner, group], index) => {
      sudo("chown", `${String(owner)}:${String(group)}`, f.asset);
      const output = join(f.output, "negative-" + String(index));
      mkdirSync(output, { mode: 0o700 });
      expect(() => stage(output)).toThrow();
      expect(existsSync(join(output, "browser"))).toBe(false);
      sudo("chown", `${String(uid())}:${String(gid())}`, f.asset);
    });
  });
  test("a symlinked source asset is refused", () => {
    unlinkSync(f.asset);
    symlinkSync(f.chrome, f.asset);
    expect(() => stage()).toThrow("nonregular browser asset");
  });
  for (const change of ["new", "missing", "changed", "unreadable", "mode", "directory"])
    test("a runtime copy with a " + change + " asset is refused", () => {
      const current = stage();
      process.env.PLAYWRIGHT_BROWSERS_PATH = join(f.output, "browser");
      expect(admittedBrowser(f.output, [uid(), gid()])).toBe(current);
      const asset = join(dirname(current), "deb.deps");
      if (change === "new") writeFileSync(join(dirname(current), "unexpected"), "new");
      else if (change === "missing") unlinkSync(asset);
      else if (change === "changed") writeFileSync(asset, "drift");
      else if (change === "mode") chmodSync(asset, 0o640);
      else if (change === "directory") mkdirSync(join(dirname(current), "unexpected-empty"));
      else chmodSync(asset, 0);
      expect(() => admittedBrowser(f.output, [uid(), gid()])).toThrow();
    });
  test("an occupied runtime browser prefix is refused", () => {
    stage();
    expect(() => stage()).toThrow();
  });
});
