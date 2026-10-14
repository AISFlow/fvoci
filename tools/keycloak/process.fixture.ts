// Actual children for the runner's exit/output/interrupt controls, not mocks.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pause, UiProcesses, withProcesses } from "../turso/ui-processes.ts";
import { addSecret } from "./redact.ts";
import { runCommand, safeText } from "./run.ts";

const [kind, mode, directory = ""] = process.argv.slice(2);
const ready = join(directory, "ready");
if (kind === "body") {
  process.stdout.write("stdout-canary-secret-012345\n");
  process.stderr.write("stderr-collected\n");
  if (mode === "wait" || mode === "descendant") {
    const descendant =
      mode === "wait"
        ? Bun.spawn(
            [process.execPath, "--no-env-file", import.meta.path, "body", "descendant", directory],
            { stdout: "ignore", stderr: "ignore", env: { PATH: process.env.PATH ?? "" } },
          )
        : undefined;
    const alive = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: () => new Response("owned child"),
    });
    process.on("SIGTERM", () => {
      void (async () => {
        await alive.stop(true);
        await descendant?.exited;
        process.stdout.write("terminated-and-drained\n");
        process.exit(0);
      })().catch(() => {
        process.exit(1);
      });
    });
    if (descendant) {
      const deadline = performance.now() + 10000;
      while (!existsSync(ready + "-descendant")) {
        if (performance.now() >= deadline) throw new Error("descendant readiness missing");
        await pause(20);
      }
    }
    writeFileSync(mode === "descendant" ? ready + "-descendant" : ready, String(process.pid));
    await new Promise(() => {});
  } else process.exit(mode === "failure" ? 7 : 0);
} else {
  addSecret("stdout-canary-secret-012345", "fixture canary");
  const controller = new AbortController();
  let captured = "",
    result = "",
    code: number | null = null;
  const scope = new UiProcesses(undefined, {
    root: () => directory,
    write: (path, value) => {
      writeFileSync(path, safeText(JSON.stringify(value)), { mode: 0o600 });
    },
    output: { out: () => {}, err: () => {} },
  });
  await withProcesses(async () => {
    const command = runCommand(
      scope,
      mode === "spawn-failure"
        ? [join(directory, "does-not-exist")]
        : [
            process.execPath,
            "--no-env-file",
            import.meta.path,
            "body",
            mode === "interrupt" ? "wait" : (mode ?? ""),
            directory,
          ],
      {
        env: { PATH: process.env.PATH ?? "" },
        signal: controller.signal,
        output: (stdout, stderr) => {
          captured = safeText(stdout + stderr);
        },
      },
    );
    // Install rejection handling before waiting on the child's ready witness.
    const observed = command.then(
      (value) => {
        code = value.code;
        result = "exited";
      },
      () => {
        result = controller.signal.aborted ? "interrupted" : "spawn-failed";
      },
    );
    if (mode === "interrupt") {
      const deadline = performance.now() + 10000;
      while (!existsSync(ready)) {
        if (performance.now() >= deadline) throw new Error("child readiness missing");
        await pause(20);
      }
      if (Number(readFileSync(ready, "utf8")) <= 0) throw new Error("child identity missing");
      controller.abort(new Error("fixture interrupt"));
    }
    await observed;
  }, scope);
  process.stdout.write(
    JSON.stringify({
      result,
      code,
      captured,
      closed: scope.allocations.every((a) => a.closed),
      identitiesRetired: [...scope.entries.values()].every((entry) =>
        scope.retired(entry.identity),
      ),
    }) + "\n",
  );
}
