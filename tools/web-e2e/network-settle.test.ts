import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  countLines,
  LIMIT_S,
  markLine,
  ProbeError,
  settle,
  tentativeInterfaces,
  type SettleHost,
} from "./network-settle.ts";

const SCRIPT = path.join(import.meta.dirname, "network-settle.ts");

describe("tentativeInterfaces", () => {
  test("takes the second field of each line, without trailing colons, sorted once", () => {
    const output = [
      "9: veth-b    inet6 fe80::1/64 scope link tentative \\       valid_lft forever",
      "3: eth0:    inet6 fe80::2/64 scope link tentative",
      "9: veth-b    inet6 fe80::3/64 scope link tentative",
      "lonely",
      "",
      "4:\tveth-a::\tinet6 fe80::4/64",
    ].join("\n");
    expect(tentativeInterfaces(output)).toEqual(["eth0", "veth-a", "veth-b"]);
    expect(tentativeInterfaces("")).toEqual([]);
  });
});

describe("countLines", () => {
  test("counts newlines and an unterminated last line", () => {
    const bytes = (text: string) => new TextEncoder().encode(text);
    expect(countLines(bytes(""))).toBe(0);
    expect(countLines(bytes("a\n"))).toBe(1);
    expect(countLines(bytes("a\nb"))).toBe(2);
    expect(countLines(bytes("\n\n"))).toBe(2);
  });
});

describe("markLine", () => {
  test("is the ip -tshort prefix with microseconds in UTC", () => {
    expect(markLine(1_767_225_600_000_042n, "settled")).toBe(
      "[2026-01-01T00:00:00.000042] # fvoci: network settle: settled\n",
    );
  });
});

type FakeOptions = {
  tentative?: (call: number) => string[] | ProbeError;
  monitor?: boolean;
  quietAt?: (now: number) => number;
};

function fakeHost(options: FakeOptions) {
  let now = 0;
  let calls = 0;
  const said: string[] = [];
  const host: SettleHost = {
    monotonic: () => now,
    listTentative() {
      calls += 1;
      const result = options.tentative?.(calls) ?? [];
      if (result instanceof ProbeError) throw result;
      return result;
    },
    monitorRunning: () => options.monitor ?? false,
    quietSeconds: () => options.quietAt?.(now) ?? 5,
    eventCount: () => 1,
    sleep(seconds) {
      now += seconds;
    },
    say(message) {
      said.push(message);
    },
  };
  return { host, said, calls: () => calls, now: () => now };
}

describe("settle", () => {
  test("returns at once when the host is quiet", () => {
    const fake = fakeHost({ monitor: true });
    settle(fake.host);
    expect(fake.said).toEqual(["settled after 0.00 s; netlink events since the group started: 1"]);
  });

  test("waits out tentative addresses", () => {
    const fake = fakeHost({ monitor: true, tentative: (call) => (call <= 3 ? ["veth0"] : []) });
    settle(fake.host);
    expect(fake.calls()).toBe(4);
    expect(fake.said).toEqual(["settled after 0.20 s; netlink events since the group started: 1"]);
  });

  test("waits for a full quiet second on the netlink log", () => {
    const fake = fakeHost({ monitor: true, quietAt: (now) => now + 0.5 });
    settle(fake.host);
    expect(fake.said).toEqual(["settled after 0.50 s; netlink events since the group started: 1"]);
  });

  test("warns and continues at the bound", () => {
    const fake = fakeHost({
      monitor: true,
      tentative: () => ["veth-b", "veth-a"],
      quietAt: () => 0.25,
    });
    settle(fake.host);
    expect(fake.now()).toBeGreaterThanOrEqual(LIMIT_S);
    expect(fake.now()).toBeLessThan(LIMIT_S + 0.2);
    expect(fake.said).toEqual([
      "warning: host network still changing after 10 s (tentative: veth-b, veth-a; last netlink event 0.25 s ago; netlink events since the group started: 1); continuing",
    ]);
  });

  test("without a monitor, checks tentative addresses only", () => {
    const fake = fakeHost({ tentative: () => ["veth0"] });
    settle(fake.host);
    expect(fake.said).toEqual([
      "netlink monitor not running; not checking for recent events",
      "warning: host network still changing after 10 s (tentative: veth0); continuing",
    ]);
  });

  test("skips when neither check can run", () => {
    const fake = fakeHost({ tentative: () => new ProbeError("ip could not start: ENOENT") });
    settle(fake.host);
    expect(fake.said).toEqual([
      "cannot list tentative addresses (ip could not start: ENOENT)",
      "netlink monitor not running; not checking for recent events",
      "skipped",
    ]);
  });

  test("an unavailable tentative query leaves the monitor check", () => {
    const fake = fakeHost({
      monitor: true,
      tentative: () => new ProbeError("ip timed out after 5 s"),
    });
    settle(fake.host);
    expect(fake.calls()).toBe(1);
    expect(fake.said).toEqual([
      "cannot list tentative addresses (ip timed out after 5 s)",
      "settled after 0.00 s; netlink events since the group started: 1",
    ]);
  });

  test("a query that fails after it once ran fails the wait", () => {
    const fake = fakeHost({
      monitor: true,
      tentative: (call) => (call > 1 ? new ProbeError("ip exited with status 1") : []),
    });
    expect(() => {
      settle(fake.host);
    }).toThrow("ip exited with status 1");
  });
});

describe("network-settle.ts CLI", () => {
  const dirs: string[] = [];
  afterEach(() => {
    for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  // A PATH holding only a fake `ip` that runs <body> for the tentative query.
  function fakeIp(body: string): { dir: string; bin: string } {
    const dir = mkdtempSync(path.join(tmpdir(), "network-settle-"));
    dirs.push(dir);
    const bin = path.join(dir, "bin");
    Bun.spawnSync(["mkdir", bin]);
    const ip = path.join(bin, "ip");
    writeFileSync(
      ip,
      `#!/bin/sh\nif [ "$*" = "-6 -o addr show tentative -dadfailed" ]; then\n${body}\nfi\necho "unexpected ip $*" >&2\nexit 9\n`,
    );
    chmodSync(ip, 0o755);
    return { dir, bin };
  }

  function run(bin: string, env: Record<string, string> = {}) {
    const result = Bun.spawnSync([process.execPath, SCRIPT], {
      env: { PATH: bin, ...env },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    });
    return {
      status: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    };
  }

  test("settles with a running monitor and appends the marks", () => {
    const { dir, bin } = fakeIp("exit 0");
    const monitorLog = path.join(dir, "net-monitor.log");
    const marksLog = path.join(dir, "net-marks.log");
    writeFileSync(monitorLog, "event one\nevent two\n");
    const past = new Date(Date.now() - 5000);
    utimesSync(monitorLog, past, past);
    const result = run(bin, {
      NET_MONITOR_LOG: monitorLog,
      NET_MARKS_LOG: marksLog,
      NET_MONITOR_PID: String(process.pid),
    });
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("");
    expect(result.stderr).toMatch(
      /^network settle: settled after \d+\.\d\d s; netlink events since the group started: 2\n$/,
    );
    expect(readFileSync(marksLog, "utf8")).toMatch(
      /^\[\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}\] # fvoci: network settle: settled after \d+\.\d\d s; netlink events since the group started: 2\n$/,
    );
  });

  test.each([
    ["0", "a zero pid"],
    ["-1", "a negative pid"],
    [" 1", "a padded pid"],
  ])("monitor pid %p (%s) is not a running monitor", (pid) => {
    const { dir, bin } = fakeIp("exit 0");
    const monitorLog = path.join(dir, "net-monitor.log");
    writeFileSync(monitorLog, "");
    const result = run(bin, { NET_MONITOR_LOG: monitorLog, NET_MONITOR_PID: pid });
    expect(result.status).toBe(0);
    expect(result.stderr).toMatch(
      /^network settle: netlink monitor not running; not checking for recent events\nnetwork settle: settled after \d+\.\d\d s\n$/,
    );
  });

  test("skips without ip and without a monitor", () => {
    const { dir } = fakeIp("exit 0");
    const result = run(path.join(dir, "empty"));
    expect(result.status).toBe(0);
    expect(result.stderr).toBe(
      "network settle: cannot list tentative addresses (ip -6 -o addr show tentative -dadfailed could not start: ENOENT)\n" +
        "network settle: netlink monitor not running; not checking for recent events\n" +
        "network settle: skipped\n",
    );
  });

  test("a query failing after its first run exits 1 and names the command and its error", () => {
    const { dir, bin } = fakeIp(
      `n=0; if [ -f "$0.calls" ]; then read n <"$0.calls"; fi; n=$((n + 1)); echo $n >"$0.calls"\nif [ $n -gt 1 ]; then echo "netlink query failed" >&2; exit 1; fi\nexit 0`,
    );
    const monitorLog = path.join(dir, "net-monitor.log");
    writeFileSync(monitorLog, "");
    const result = run(bin, { NET_MONITOR_LOG: monitorLog, NET_MONITOR_PID: String(process.pid) });
    expect(result.status).toBe(1);
    expect(result.stderr).toBe(
      "network settle: error: ip -6 -o addr show tentative -dadfailed exited with status 1: netlink query failed\n",
    );
  });

  test("output that is not UTF-8 fails the check", () => {
    const { bin } = fakeIp(String.raw`printf '9: veth\377 inet6 fe80::1/64 tentative\n'; exit 0`);
    const result = run(bin);
    expect(result.status).toBe(1);
    expect(result.stderr).toStartWith("network settle: error: ");
  });

  test("refuses arguments", () => {
    const { bin } = fakeIp("exit 0");
    const result = Bun.spawnSync([process.execPath, SCRIPT, "extra"], {
      env: { PATH: bin },
      stderr: "pipe",
    });
    expect(result.exitCode).toBe(2);
  });
});
