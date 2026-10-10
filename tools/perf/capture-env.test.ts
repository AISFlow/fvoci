import { describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  collect,
  cpuModel,
  lineCount,
  memTotal,
  osName,
  playwrightEnv,
  renderEnvironment,
  run,
  type Probes,
} from "./capture-env.ts";

const probes: Probes = {
  gitHead: "0123456789abcdef0123456789abcdef01234567",
  gitStatus: " M a\n?? b",
  kernel: "6.1.0",
  osRelease: 'NAME="Debian"\nPRETTY_NAME="Debian GNU/Linux 13 (trixie)"\n',
  lscpu: "Architecture:  x86_64\nModel name:     AMD EPYC: 7B13\nThread(s) per core: 2",
  logicalCpus: 8,
  meminfo: "MemTotal:       16384000 kB\nMemFree:  1 kB\n",
  loadavg: [0.5, 1.25, 2],
  dockerServer: "29.0.0",
  dockerInfo: "8 cpus 16777216000 bytes",
  bun: "1.4.2",
  playwright: "Version 1.63.0",
  serverBytes: 10,
  collabBytes: 20,
  rustc: "unavailable: ENOENT",
};

describe("renderEnvironment", () => {
  test("keeps the evidence keys, order and indent=1 layout", () => {
    const text = renderEnvironment(probes);
    expect(text.endsWith("}\n")).toBe(true);
    expect(text.split("\n")[1]).toBe(' "git_head": "0123456789abcdef0123456789abcdef01234567",');
    expect(Object.keys(JSON.parse(text) as object)).toEqual([
      "git_head",
      "git_dirty_paths",
      "build_kind",
      "published_image_digest",
      "kernel",
      "os",
      "cpu_model",
      "logical_cpus",
      "mem_total",
      "loadavg_at_start",
      "docker_server",
      "docker_ncpu_mem",
      "docker_container_limits",
      "bun",
      "playwright",
      "server_binary_bytes",
      "collab_engine_binary_bytes",
      "rustc",
    ]);
    expect(JSON.parse(text)).toMatchObject({
      git_dirty_paths: 2,
      published_image_digest: null,
      os: "Debian GNU/Linux 13 (trixie)",
      cpu_model: "AMD EPYC: 7B13",
      mem_total: "16384000 kB",
      loadavg_at_start: [0.5, 1.25, 2],
      rustc: "unavailable: ENOENT",
    });
  });

  test("records host facts only, never environment values", () => {
    const text = renderEnvironment(probes);
    for (const value of Object.values(process.env)) {
      if (value && value.length >= 12) expect(text.includes(value)).toBe(false);
    }
  });
});

describe("field parsers", () => {
  test("missing fields fall back like the evidence format expects", () => {
    expect(cpuModel("Architecture: x86_64")).toBe("");
    expect(osName("NAME=x\n")).toBe("");
    expect(osName("PRETTY_NAME=Plain\n")).toBe("Plain");
    expect(memTotal("MemFree: 1 kB")).toBeNull();
    expect(lineCount("")).toBe(0);
    expect(lineCount("one")).toBe(1);
    expect(lineCount(null)).toBeNull();
  });
});

describe("run", () => {
  test("keeps trimmed stdout of a successful command", () => {
    expect(run(["sh", "-c", "echo '  out  '; echo err >&2"])).toBe("out");
  });

  test("records a non-zero exit as unavailable, not as its partial stdout", () => {
    expect(run(["sh", "-c", "echo partial; exit 3"])).toBe("unavailable: exit 3");
  });

  test("records death by signal as unavailable", () => {
    expect(run(["sh", "-c", "kill -TERM $$"])).toBe("unavailable: signal SIGTERM");
  });

  test("a failed git status gives no dirty-path count", () => {
    const dir = mkdtempSync(join(tmpdir(), "capture-env-"));
    try {
      writeFileSync(join(dir, "server"), "12345");
      const text = renderEnvironment(collect(dir, join(dir, "server"), join(dir, "server")));
      const record = JSON.parse(text) as Record<string, unknown>;
      expect(record.git_dirty_paths).toBeNull();
      expect(record.git_head).toBe("unavailable: exit 128");
      expect(record.server_binary_bytes).toBe(5);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a missing binary fails the capture", () => {
    expect(() => collect("/", "/nonexistent/fvoci-server", "/nonexistent/collab")).toThrow();
  });

  test("records a missing executable as unavailable", () => {
    expect(run(["fvoci-no-such-command-for-capture-env"])).toBe("unavailable: ENOENT");
  });

  test("records a timeout as unavailable", () => {
    expect(run(["sleep", "5"], { timeoutMs: 100 })).toBe("unavailable: ETIMEDOUT");
  });

  test("runs in the requested directory", () => {
    const dir = mkdtempSync(join(tmpdir(), "capture-env-"));
    try {
      writeFileSync(join(dir, "marker"), "");
      expect(run(["ls"], { cwd: dir })).toBe("marker");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("Bun and Playwright children", () => {
  test("playwrightEnv drops only JEST_WORKER_ID", () => {
    expect(playwrightEnv({ JEST_WORKER_ID: "3", PATH: "/bin", HOME: "/h" })).toEqual({
      PATH: "/bin",
      HOME: "/h",
    });
  });

  test("the Playwright probe re-launches the given Bun without JEST_WORKER_ID", () => {
    const dir = mkdtempSync(join(tmpdir(), "capture-env-"));
    const saved = process.env.JEST_WORKER_ID;
    try {
      mkdirSync(join(dir, "apps/web"), { recursive: true });
      writeFileSync(join(dir, "server"), "x");
      const fake = join(dir, "fake-bun");
      writeFileSync(
        fake,
        `#!/bin/sh\necho "$* jest=\${JEST_WORKER_ID-unset}" >>'${dir}/calls'\necho 9.9.9\n`,
      );
      chmodSync(fake, 0o755);
      process.env.JEST_WORKER_ID = "7";
      const record = collect(dir, join(dir, "server"), join(dir, "server"), fake);
      expect(record.bun).toBe("9.9.9");
      expect(readFileSync(join(dir, "calls"), "utf8").trimEnd().split("\n")).toEqual([
        "--version jest=7",
        "--bun x --no-install playwright --version jest=unset",
      ]);
    } finally {
      if (saved === undefined) delete process.env.JEST_WORKER_ID;
      else process.env.JEST_WORKER_ID = saved;
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("by default Bun children use process.execPath, not a PATH lookup", () => {
    const dir = mkdtempSync(join(tmpdir(), "capture-env-"));
    const savedPath = process.env.PATH;
    try {
      writeFileSync(join(dir, "server"), "x");
      writeFileSync(join(dir, "bun"), `#!/bin/sh\ntouch '${dir}/path-bun-used'\n`);
      chmodSync(join(dir, "bun"), 0o755);
      process.env.PATH = `${dir}:${savedPath ?? ""}`;
      const record = collect(dir, join(dir, "server"), join(dir, "server"));
      expect(existsSync(join(dir, "path-bun-used"))).toBe(false);
      expect(record.bun).toBe(Bun.version);
    } finally {
      process.env.PATH = savedPath;
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
