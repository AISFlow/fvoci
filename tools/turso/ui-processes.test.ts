import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { write } from "../selected-backend-ci/io.ts";
import { UiError } from "./ui-common.ts";
import { Captured, exitedChild, failureOf, must } from "./ui-fakes.ts";
import {
  linuxKernel,
  procIdentity,
  UiProcesses,
  withProcesses,
  type Child,
  type Kernel,
  type ProcRow,
} from "./ui-processes.ts";

let directory: string;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "fvoci-ui-processes-"));
});
afterEach(() => {
  rmSync(directory, { recursive: true, force: true });
  UiProcesses.active = null;
});

const missing = () => Object.assign(new Error("gone"), { code: "ENOENT" });
function fakeKernel(rows: Map<number, ProcRow>, overrides: Partial<Kernel> = {}) {
  const signals: [number, number][] = [];
  const closed: number[] = [];
  let subreaper = 0;
  const kernel: Kernel = {
    procRows: () => [...rows.values()].map((row) => ({ ...row })),
    procIdentity: (pid) => {
      const row = rows.get(pid);
      if (!row) throw missing();
      return { ...row };
    },
    pidfdOpen: (pid) => pid + 1000,
    pidfdSendSignal: (fd, signal) => signals.push([fd, signal]),
    close: (fd) => closed.push(fd),
    reap: (pid) => pid,
    getSubreaper: () => subreaper,
    setSubreaper: (value) => {
      subreaper = value;
    },
    spawn: () => {
      throw new Error("unexpected spawn");
    },
    ...overrides,
  };
  return { kernel, signals, closed };
}
const io = (output = new Captured()) => ({ output, write, root: () => directory });

describe("process history", () => {
  test("covers detach, adoption and PID reuse and excludes foreign PIDs", () => {
    const rows = new Map<number, ProcRow>([
      [10, { pid: 10, parentPid: 999, startTicks: "100", state: "S" }],
      [11, { pid: 11, parentPid: 10, startTicks: "101", state: "S" }],
      [12, { pid: 12, parentPid: 11, startTicks: "102", state: "S" }],
      [77, { pid: 77, parentPid: 88, startTicks: "777", state: "S" }],
    ]);
    const { kernel, signals } = fakeKernel(rows);
    const tracker = new UiProcesses(kernel, io());
    tracker.pid = 999;
    tracker.allocations = [
      {
        process: { pid: 10 } as Child,
        label: "server",
        closed: false,
        forced: false,
        key: "10:100",
      },
    ];
    tracker.capture(must(rows.get(10)), "server", 0);
    tracker.snapshot();
    expect(new Set([...tracker.entries.values()].map((e) => e.identity.pid))).toEqual(
      new Set([10, 11, 12]),
    );
    must(rows.get(12)).parentPid = 999; // a detached grandchild reparented to the subreaper
    rows.set(13, { pid: 13, parentPid: 999, startTicks: "103", state: "S" }); // unobserved adoptee
    tracker.snapshot();
    expect(must(tracker.entries.get("12:102")).allocation).toBe(0);
    expect(must(tracker.entries.get("13:103")).allocation).toBeNull();
    const old = must(tracker.entries.get("11:101"));
    rows.set(11, { pid: 11, parentPid: 88, startTicks: "500", state: "S" }); // unrelated reuse
    tracker.send(old, 15);
    expect(signals).toEqual([]);
    tracker.send(must(tracker.entries.get("12:102")), 15);
    expect(signals).toEqual([[1012, 15]]);
    expect(tracker.entries.has("77:777")).toBe(false);
    expect(tracker.closure()).toBe(false);
    must(tracker.allocations[0]).closed = true;
    rows.clear();
    expect(tracker.closure()).toBe(true);
  });

  test("an adopted zombie is reaped, a root child is left to its own wait", () => {
    const rows = new Map<number, ProcRow>([
      [10, { pid: 10, parentPid: 999, startTicks: "100", state: "Z" }],
      [12, { pid: 12, parentPid: 999, startTicks: "102", state: "Z" }],
    ]);
    const reaped: number[] = [];
    const { kernel } = fakeKernel(rows, { reap: (pid) => (reaped.push(pid), pid) });
    const tracker = new UiProcesses(kernel, io());
    tracker.pid = 999;
    tracker.allocations = [
      { process: { pid: 10 } as Child, label: "server", closed: false, forced: false },
    ];
    tracker.capture(must(rows.get(10)), "server", 0);
    tracker.snapshot();
    expect(reaped).toEqual([12]);
    expect(must(tracker.entries.get("12:102")).reaped).toBe(true);
  });

  test("a pidfd capture race and a signal fault cannot qualify retirement", () => {
    const row = { pid: 10, parentPid: 999, startTicks: "100", state: "S" };
    const rows = new Map([[10, row]]);
    const closed: number[] = [];
    const racing = fakeKernel(rows, {
      pidfdOpen: () => 1010,
      procIdentity: () => ({ ...row, startTicks: "200" }),
      close: (fd) => closed.push(fd),
    });
    const tracker = new UiProcesses(racing.kernel, io());
    tracker.pid = 999;
    tracker.allocations = [
      { process: { pid: 10 } as Child, label: "server", closed: false, forced: false },
    ];
    expect(() => tracker.capture(row, "server", 0)).toThrow("UI_PROCESS_IDENTITY_RACE");
    expect(closed).toEqual([1010]);
    const faulty = fakeKernel(rows, {
      pidfdOpen: () => 1010,
      pidfdSendSignal: () => {
        throw new Error("private control");
      },
    });
    const second = new UiProcesses(faulty.kernel, io());
    second.pid = 999;
    second.allocations = tracker.allocations;
    second.capture(row, "server", 0);
    expect(() => {
      second.send(must(second.entries.get("10:100")), 15);
    }).toThrow("private control");
    expect(second.closure()).toBe(false);
  });

  test("a vanished process is captured without a pidfd only when retired", () => {
    const row = { pid: 10, parentPid: 999, startTicks: "100", state: "S" };
    const gone = fakeKernel(new Map(), {
      pidfdOpen: () => {
        throw missing();
      },
    });
    const tracker = new UiProcesses(gone.kernel, io());
    tracker.capture(row, "server", 0);
    expect(must(tracker.entries.get("10:100")).pidfd).toBeNull();
    const alive = fakeKernel(new Map([[10, row]]), {
      pidfdOpen: () => {
        throw missing();
      },
    });
    expect(() => new UiProcesses(alive.kernel, io()).capture(row, "server", 0)).toThrow(
      "UI_PROCESS_IDENTITY_UNCONFIRMED",
    );
  });
});

describe("subreaper scope", () => {
  test("verifies and restores the prior attribute without a real prctl", async () => {
    for (const prior of [0, 1]) {
      let flag = prior;
      const calls: string[] = [];
      const { kernel } = fakeKernel(new Map(), {
        getSubreaper: () => (calls.push("get"), flag),
        setSubreaper: (value) => {
          calls.push("set");
          flag = value;
        },
      });
      const scope = new UiProcesses(kernel, io()).open();
      expect(flag).toBe(1);
      expect(scope.observing).toBe(true);
      expect(UiProcesses.active).toBe(scope);
      await scope.close();
      expect(flag).toBe(prior);
      expect(calls).toEqual(["get", "set", "get", "set", "get"]);
      expect(scope.observing).toBe(false);
      expect(UiProcesses.active).toBeNull();
      expect(readdirSync(directory).filter((n) => n.startsWith("process-closure-"))).toHaveLength(
        1,
      );
      for (const name of readdirSync(directory)) rmSync(join(directory, name));
    }
  });

  test("a second scope, a preexisting child or an unconfirmed attribute is refused", () => {
    const { kernel } = fakeKernel(new Map());
    new UiProcesses(kernel, io()).open();
    expect(() => new UiProcesses(kernel, io()).open()).toThrow("UI_PROCESS_CAPABILITY_REQUIRED");
    UiProcesses.active = null;
    const child = fakeKernel(
      new Map([[5, { pid: 5, parentPid: process.pid, startTicks: "1", state: "S" }]]),
    );
    expect(() => new UiProcesses(child.kernel, io()).open()).toThrow(
      "UI_PREEXISTING_CHILD_REFUSED",
    );
    let flag = 0;
    const stuck = fakeKernel(new Map(), {
      getSubreaper: () => flag,
      setSubreaper: (value) => {
        if (value === 0) flag = 0;
      },
    });
    expect(() => new UiProcesses(stuck.kernel, io()).open()).toThrow("UI_SUBREAPER_NOT_CONFIRMED");
    expect(UiProcesses.active).toBeNull();
  });

  test("without a usable pidfd the scope is refused before the subreaper is set", async () => {
    const fault = () => {
      throw new Error("pidfd syscall failed");
    };
    for (const overrides of [
      { pidfdOpen: fault },
      { pidfdSendSignal: fault },
      { close: fault },
    ] as Partial<Kernel>[]) {
      const calls: string[] = [];
      const { kernel } = fakeKernel(new Map(), {
        ...overrides,
        getSubreaper: () => (calls.push("get"), 0),
        setSubreaper: () => calls.push("set"),
      });
      expect(() => new UiProcesses(kernel, io()).open()).toThrow("UI_PROCESS_CAPABILITY_REQUIRED");
      expect(calls).toEqual([]);
      expect(UiProcesses.active).toBeNull();
    }
    const closed: number[] = [];
    const probe = fakeKernel(new Map(), { close: (fd) => closed.push(fd) });
    const scope = new UiProcesses(probe.kernel, io()).open();
    expect(probe.signals).toEqual([[scope.pid + 1000, 0]]);
    expect(closed).toEqual([scope.pid + 1000]);
    await scope.close();
    expect(UiProcesses.active).toBeNull();
  });

  test("an unknown closure keeps the original failure and does not restore the flag", async () => {
    const restored: number[] = [];
    const { kernel } = fakeKernel(new Map(), { setSubreaper: (value) => restored.push(value) });
    const output = new Captured();
    const scope = new UiProcesses(kernel, {
      output,
      write: () => {
        throw new Error("PRIVATE_FAKE");
      },
      root: () => directory,
    });
    scope.errors.push("UI_PROCESS_OBSERVATION_FAILED");
    await scope.close(new UiError("UI_ACTUAL_BROWSER_FAILED"));
    expect(restored).toEqual([]);
    expect(output.all).toContain("UI_ACTUAL_BROWSER_FAILED");
    expect(output.all).toContain("UI_PROCESS_RECEIPT_WRITE_FAILED");
    expect(output.all).not.toContain("PRIVATE_FAKE");
  });

  test("final observation and cleanup faults keep the original and an unknown closure", async () => {
    for (const fault of ["snapshot", "stop", "descriptor", "receipt"]) {
      const output = new Captured();
      const attempted: unknown[] = [];
      const closed: number[] = [];
      const restored: number[] = [];
      const { kernel } = fakeKernel(new Map(), {
        procRows: () => {
          throw new UiError("UI_PROCESS_SNAPSHOT_CAP_REFUSED");
        },
        close: (fd) => {
          closed.push(fd);
          if (fault === "descriptor") throw new Error("PRIVATE_FD");
        },
        setSubreaper: (value) => restored.push(value),
      });
      const scope = new UiProcesses(kernel, {
        output,
        write: (path, value) => {
          attempted.push(value);
          if (fault === "receipt") throw new Error("PRIVATE_RECEIPT");
          write(path, value);
        },
        root: () => directory,
      });
      scope.allocations = [
        { process: exitedChild(0, "", 10), label: "x", closed: false, forced: false },
      ];
      scope.entries.set("synthetic", {
        pidfd: 123,
        identity: { pid: 10, parentPid: 1, startTicks: "1", state: "S" },
        allocation: 0,
        label: "x",
        reaped: false,
      });
      if (fault !== "snapshot") scope.closure = () => false;
      if (fault === "stop")
        scope.stopWatch = () => {
          throw new Error("PRIVATE_STOP");
        };
      await scope.close(new UiError("UI_ACTUAL_BROWSER_FAILED"));
      expect(closed).toEqual([123]);
      expect(restored).toEqual([]);
      expect(attempted).toHaveLength(1);
      expect((attempted[0] as { confirmed: boolean }).confirmed).toBe(false);
      expect(output.all).toContain("UI_ACTUAL_BROWSER_FAILED");
      expect(output.all).not.toContain("PRIVATE_");
      const expected = must(
        {
          snapshot: "UI_PROCESS_FINAL_OBSERVATION_FAILED",
          stop: "UI_PROCESS_OBSERVER_STOP_FAILED",
          descriptor: "UI_PIDFD_CLOSE_FAILED",
          receipt: "UI_PROCESS_RECEIPT_WRITE_FAILED",
        }[fault],
      );
      expect(output.all).toContain(expected);
      for (const name of readdirSync(directory)) rmSync(join(directory, name));
    }
  });
});

// Real Linux children: subreaper, pidfd signals and reaping, no mocks.
describe.skipIf(process.platform !== "linux")("real owned children", () => {
  test("a normal child closes, its receipt is written and the attribute is restored", async () => {
    const prior = linuxKernel.getSubreaper();
    const code = await withProcesses(
      async (scope) => {
        const child = scope.spawn(["sh", "-c", "exit 3"], "probe", {
          env: { PATH: process.env.PATH ?? "" },
        });
        return scope.finish(child);
      },
      new UiProcesses(linuxKernel, io()),
    );
    expect(code).toBe(3);
    expect(linuxKernel.getSubreaper()).toBe(prior);
    const name = must(readdirSync(directory).find((n) => n.startsWith("process-closure-")));
    const receipt = JSON.parse(readFileSync(join(directory, name), "utf8")) as {
      confirmed: boolean;
      normalClosure: boolean;
    };
    expect(receipt).toMatchObject({ confirmed: true, normalClosure: true });
  });

  test("a detached grandchild is adopted, killed through its pidfd and the closure fails", async () => {
    const output = new Captured();
    let grandchild = 0;
    const result = withProcesses(
      async (scope) => {
        const child = scope.spawn(
          ["sh", "-c", "setsid sleep 60 >/dev/null 2>&1 & echo $!"],
          "probe",
          {
            env: { PATH: process.env.PATH ?? "" },
            stdout: "pipe",
          },
        );
        grandchild = Number((await new Response(child.stdout as ReadableStream).text()).trim());
        await scope.finish(child);
      },
      new UiProcesses(linuxKernel, io(output)),
    );
    expect(await failureOf(result)).toContain("UI_PROCESS_CLOSURE_FAILED");
    expect(() => procIdentity(grandchild)).toThrow();
    expect(UiProcesses.active).toBeNull();
  }, 60000);

  test("a spawn failure is the original failure and leaves no allocation open", async () => {
    await failureOf(
      withProcesses(
        (scope) => {
          scope.spawn(["/nonexistent/fvoci-binary"], "probe", { env: {} });
          return Promise.resolve();
        },
        new UiProcesses(linuxKernel, io()),
      ),
    );
    expect(UiProcesses.active).toBeNull();
  });
});
