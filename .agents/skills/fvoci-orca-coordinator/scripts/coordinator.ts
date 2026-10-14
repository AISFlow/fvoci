import {
  constants,
  lstatSync,
  openSync,
  closeSync,
  readFileSync,
  writeFileSync,
  unlinkSync,
  realpathSync,
  existsSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, relative } from "node:path";

type Model = { model: string; effort: string; tier: string; banner: string };
export type Profile = {
  version: number;
  cli: string;
  codex: string;
  repo: string;
  parent: string;
  parentId: string;
  parentInstance: string;
  run: string;
  coordinator: string;
  handoff: string;
  state: string;
  scratch: string;
  heavy: string;
  authorCommon: string;
  reviewerCommon: string;
  controller: Model;
  worker: Model;
  concurrency: number;
};
export type Request = {
  role: "author" | "reviewer";
  unit: string;
  branch: string;
  sha: string;
  worktree: string;
  prompt: string;
  report: string;
  spec: string;
  name: string;
  terminal: string | null;
};
type Receipt = { ok: boolean; result: any };
type Effect = { kind?: string; role?: string; id?: string; state?: string };
type FleetRow = {
  dispatchId?: string;
  agentTerminalHandle?: string;
  dispatchStatus?: string;
  terminalState?: string;
  projection?: {
    taskId?: string;
    outcome?: string;
    liveness?: unknown;
    attention?: unknown;
    nextAction?: unknown;
  };
};
class ChildFailure extends Error {
  constructor(
    message: string,
    readonly stdout: string,
  ) {
    super(message);
  }
}
export type Exec = (argv: string[]) => Promise<string>;
export function requireThat(value: unknown, why: string): asserts value {
  if (!value) throw new Error(why);
}
function exactKeys(value: any, keys: string[]) {
  requireThat(value && typeof value === "object" && !Array.isArray(value), "expected object");
  requireThat(Object.keys(value).sort().join() === keys.sort().join(), "unexpected/missing keys");
}
export function privateJson(path: string): any {
  const st = lstatSync(path);
  requireThat(
    st.isFile() &&
      !st.isSymbolicLink() &&
      (st.mode & 0o777) === 0o600 &&
      st.uid === process.getuid?.(),
    "private file must be owned regular mode 0600",
  );
  return JSON.parse(readFileSync(path, "utf8"));
}
export function loadProfile(
  path = join(
    process.env.XDG_STATE_HOME || join(homedir(), ".local/state"),
    "fvoci-orca/current.json",
  ),
): Profile {
  const p = privateJson(path);
  exactKeys(p, [
    "version",
    "cli",
    "codex",
    "repo",
    "parent",
    "parentId",
    "parentInstance",
    "run",
    "coordinator",
    "handoff",
    "state",
    "scratch",
    "heavy",
    "authorCommon",
    "reviewerCommon",
    "controller",
    "worker",
    "concurrency",
  ]);
  requireThat(
    p.version === 1 && Number.isInteger(p.concurrency) && p.concurrency > 0,
    "invalid profile version/concurrency",
  );
  for (const key of [
    "cli",
    "codex",
    "repo",
    "parent",
    "handoff",
    "state",
    "scratch",
    "heavy",
    "authorCommon",
    "reviewerCommon",
  ]) {
    requireThat(
      typeof p[key] === "string" && isAbsolute(p[key]) && existsSync(p[key]),
      `missing absolute profile path: ${key}`,
    );
  }
  for (const key of ["parentId", "parentInstance", "run", "coordinator"])
    requireThat(typeof p[key] === "string" && p[key].length > 0, `missing reference: ${key}`);
  for (const key of ["controller", "worker"]) {
    exactKeys(p[key], ["model", "effort", "tier", "banner"]);
    for (const value of Object.values(p[key]))
      requireThat(
        typeof value === "string" && /^[a-zA-Z0-9. -]+$/.test(value),
        "invalid model profile",
      );
    requireThat(
      p[key].model === p[key].banner.toLowerCase() &&
        /^(low|medium|high|xhigh|max|ultra)$/.test(p[key].effort),
      "model/banner or effort mismatch",
    );
  }
  return p;
}

// Do not print argv, command output, or the environment on failure: prompts may contain private data.
export const execute: Exec = async (argv) => {
  const child = Bun.spawn(argv, { stdout: "pipe", stderr: "pipe" });
  let interrupted = false;
  const stop = () => {
    interrupted = true;
    child.kill("SIGTERM");
  };
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  try {
    const [out, , exit] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);
    if (interrupted || exit !== 0)
      throw new ChildFailure(
        `child failed (exit ${exit}${interrupted ? ", interrupted" : ""}); receipt unknown; inspect with Orca before any retry`,
        out,
      );
    return out;
  } finally {
    process.removeListener("SIGINT", stop);
    process.removeListener("SIGTERM", stop);
  }
};
export function quoteArg(value: string): string {
  requireThat(!value.includes("\0") && !/[\r\n]/.test(value), "invalid command argument");
  return "'" + value.replaceAll("'", "'\\''") + "'";
}
export function assertBanner(text: string, m: Model) {
  const lines = text.split("\n").filter((line) => line.includes(m.banner));
  const expected = new RegExp(
    `${m.banner.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")} ${m.effort}(?:\\s|[·|]|$)`,
  );
  requireThat(
    m.model === m.banner.toLowerCase() &&
      lines.length > 0 &&
      lines.some((line) => expected.test(line)),
    "wrong/missing model or effort banner",
  );
  requireThat(
    m.tier === "default" &&
      !lines.some((line) => {
        const match = expected.exec(line);
        return match && /^\s*(fast|ultrafast)\b/i.test(line.slice(match.index + match[0].length));
      }),
    "Fast/tier mismatch",
  );
}
export function inside(root: string, path: string) {
  const rel = relative(realpathSync(root), path);
  requireThat(
    isAbsolute(path) && rel !== "" && !rel.startsWith("..") && !isAbsolute(rel),
    "path outside owned scratch",
  );
  requireThat(realpathSync(dirname(path)) === dirname(path), "noncanonical parent directory");
}
export function ownedDirectory(path: string) {
  const st = lstatSync(path);
  requireThat(
    st.isDirectory() && !st.isSymbolicLink() && st.uid === process.getuid?.(),
    "directory must be owned and not symlink",
  );
  requireThat(realpathSync(path) === path, "noncanonical owned directory");
}
export function exclusive(path: string, data: string) {
  const fd = openSync(path, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600);
  try {
    writeFileSync(fd, data);
  } finally {
    closeSync(fd);
  }
}

export class Coordinator {
  constructor(
    readonly p: Profile,
    readonly exec: Exec = execute,
  ) {}
  async cli(...args: string[]): Promise<any> {
    let out: string;
    try {
      out = await this.exec([this.p.cli, ...args, "--json"]);
    } catch (error) {
      if (error instanceof ChildFailure) {
        let receipt: any;
        try {
          receipt = JSON.parse(error.stdout);
        } catch {
          throw error;
        }
        const stage = receipt.result?.failedStage ?? receipt.result?.stage;
        if (typeof stage === "string" && /^[a-z_]+$/.test(stage)) {
          throw new Error(
            `Orca failed at ${stage}; residual resource count ${Array.isArray(receipt.result?.residualResources) ? receipt.result.residualResources.length : "unknown"}; preserve owned resources, inspect Orca recovery, never retry automatically`,
          );
        }
      }
      throw error;
    }
    let receipt: Receipt;
    try {
      receipt = JSON.parse(out);
    } catch {
      throw new Error("invalid/partial Orca receipt; preserve resources and inspect");
    }
    requireThat(
      receipt.ok === true && receipt.result && typeof receipt.result === "object",
      "Orca refusal/unknown receipt; preserve resources and inspect",
    );
    return receipt.result;
  }
  async worktree(path: string) {
    const r = await this.cli("worktree", "show", "--worktree", `path:${path}`);
    requireThat(r.worktree?.path === path, "worktree receipt path mismatch");
    return r.worktree;
  }
  async context() {
    const status = await this.cli("status");
    requireThat(
      status.runtime?.reachable === true && status.runtime?.state === "ready",
      "runtime not ready",
    );
    const run = (await this.cli("orchestration", "run-show", "--id", this.p.run)).run;
    requireThat(
      run?.id === this.p.run && run.coordinator_handle === this.p.coordinator,
      "Run/coordinator mismatch",
    );
    const parent = await this.worktree(this.p.parent);
    requireThat(
      parent.id === this.p.parentId && parent.instanceId === this.p.parentInstance,
      "parent identity mismatch",
    );
    requireThat(
      typeof parent.repoId === "string" && parent.id === parent.repoId + "::" + this.p.parent,
      "parent repository mapping missing/ambiguous",
    );
    const source = await this.worktree(this.p.repo);
    requireThat(
      source.repoId === parent.repoId && source.id === parent.repoId + "::" + this.p.repo,
      "source repository mismatch",
    );
    const repo = (await this.cli("repo", "show", "--repo", "id:" + parent.repoId)).repo;
    requireThat(
      repo?.id === parent.repoId &&
        typeof repo.path === "string" &&
        isAbsolute(repo.path) &&
        existsSync(repo.path),
      "registered repository mismatch",
    );
    const common = async (path: string) =>
      realpathSync(
        (
          await this.exec([
            "git",
            "-C",
            path,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
          ])
        ).trim(),
      );
    requireThat(
      (await common(this.p.repo)) === (await common(repo.path)) &&
        (await common(this.p.parent)) === (await common(repo.path)),
      "Git/Orca repository mapping mismatch",
    );
    const terminals = (await this.cli("terminal", "list", "--worktree", `path:${this.p.parent}`))
      .terminals;
    requireThat(
      Array.isArray(terminals) &&
        terminals.some(
          (t) =>
            t.handle === this.p.coordinator &&
            t.worktreePath === this.p.parent &&
            t.connected === true,
        ),
      "coordinator terminal missing or misplaced",
    );
    const screen = (await this.cli("terminal", "read", "--terminal", this.p.coordinator)).terminal;
    requireThat(
      screen?.handle === this.p.coordinator && Array.isArray(screen.tail),
      "missing controller screen",
    );
    assertBanner(screen.tail.join("\n"), this.p.controller);
    return { run, parent, repo };
  }
  async inspect() {
    const { run, parent } = await this.context();
    const workers: any[] = [];
    let cursor: string | undefined;
    const seen = new Set<string>();
    do {
      const args = ["orchestration", "worker-list", "--run", this.p.run, "--include-remote"];
      if (cursor) args.push("--cursor", cursor);
      const page = await this.cli(...args);
      requireThat(page.scope?.run === this.p.run, "worker scope mismatch");
      requireThat(Array.isArray(page.workers), "missing worker list");
      workers.push(
        ...page.workers.map((w: FleetRow) => ({
          dispatchId: w.dispatchId,
          terminal: w.agentTerminalHandle,
          dispatchStatus: w.dispatchStatus,
          terminalState: w.terminalState,
          taskId: w.projection?.taskId,
          outcome: w.projection?.outcome,
          liveness: w.projection?.liveness,
          attention: w.projection?.attention,
          nextAction: w.projection?.nextAction,
        })),
      );
      cursor = page.page?.hasMore ? page.page.nextCursor : undefined;
      if (page.page?.hasMore) {
        requireThat(typeof cursor === "string" && !seen.has(cursor), "invalid worker page cursor");
        seen.add(cursor);
      }
    } while (cursor);
    return {
      run: run.id,
      coordinator: run.coordinator_handle,
      parent: parent.id,
      handoff: this.p.handoff,
      state: this.p.state,
      controller: this.p.controller,
      worker: this.p.worker,
      concurrency: this.p.concurrency,
      heavy: this.p.heavy,
      workers,
    };
  }
  controllerOnly() {
    requireThat(
      process.env.ORCA_TERMINAL_HANDLE === this.p.coordinator,
      "current caller is not the verified coordinator; takeover is separate",
    );
  }
  async clean(path: string, sha?: string) {
    requireThat(
      (await this.exec(["git", "-C", path, "status", "--porcelain"])).trim() === "",
      "dirty checkout",
    );
    if (sha)
      requireThat(
        (await this.exec(["git", "-C", path, "rev-parse", "HEAD"])).trim() === sha,
        "wrong checkout SHA",
      );
  }
  async lineage(path: string) {
    const child = await this.worktree(path);
    requireThat(
      child.repoId === (await this.worktree(this.p.parent)).repoId &&
        child.parentWorktreeId === this.p.parentId &&
        child.lineage?.parentWorktreeInstanceId === this.p.parentInstance,
      "wrong/missing parent lineage",
    );
    const parent = await this.worktree(this.p.parent);
    requireThat(
      parent.instanceId === this.p.parentInstance && parent.childWorktreeIds?.includes(child.id),
      "parent reverse lineage missing",
    );
  }
  async launch(r: Request) {
    this.controllerOnly();
    requireThat(process.platform !== "win32", "command-text adapter requires POSIX host");
    exactKeys(r, [
      "role",
      "unit",
      "branch",
      "sha",
      "worktree",
      "prompt",
      "report",
      "spec",
      "name",
      "terminal",
    ]);
    requireThat(
      ["author", "reviewer"].includes(r.role) && /^[a-z0-9][a-z0-9-]*$/.test(r.unit),
      "invalid role/unit",
    );
    requireThat(
      /^draft\/[a-zA-Z0-9/_-]+$/.test(r.branch) && /^[a-f0-9]{40}$/.test(r.sha),
      "require draft branch and exact SHA40",
    );
    requireThat(
      isAbsolute(r.worktree) && /^[a-z0-9][a-z0-9-]*$/.test(r.name),
      "invalid worktree/name",
    );
    requireThat(
      typeof r.spec === "string" &&
        r.spec.length > 0 &&
        (r.terminal === null || /^term_[a-z0-9-]+$/.test(r.terminal)),
      "invalid spec/terminal",
    );
    requireThat(this.p.worker.tier === "default", "unsupported tier");
    const unitDir = join(this.p.scratch, r.unit);
    ownedDirectory(unitDir);
    inside(unitDir, r.prompt);
    inside(unitDir, r.report);
    requireThat(
      realpathSync(r.prompt) === r.prompt && lstatSync(r.prompt).isFile(),
      "prompt must be canonical regular file",
    );
    requireThat(!existsSync(r.report), "report already exists; inspect previous attempt");
    const dispatchPrompt = join(unitDir, "dispatch-prompt.md");
    requireThat(
      r.spec.includes(dispatchPrompt) && r.spec.includes(r.branch),
      "spec must reference the combined dispatch prompt",
    );
    const { repo } = await this.context();
    await this.clean(this.p.repo);
    requireThat(
      (await this.exec(["git", "-C", this.p.repo, "rev-parse", `${r.sha}^{commit}`])).trim() ===
        r.sha,
      "source commit missing",
    );
    const existed = existsSync(r.worktree);
    if (existed) {
      ownedDirectory(r.worktree);
      await this.clean(r.worktree, r.sha);
      await this.lineage(r.worktree);
    }
    if (r.terminal) {
      const prior = (await this.inspect()).workers.filter((w) => w.terminal === r.terminal);
      requireThat(
        prior.length > 0 &&
          prior.every(
            (w) =>
              w.dispatchStatus === "completed" &&
              ["succeeded", "failed"].includes(w.outcome || "") &&
              w.terminalState === "retained",
          ),
        "existing agent lacks settled retained ownership; inspect before reuse",
      );
    }
    // Never remove this marker automatically: a failed mutation may have created resources.
    exclusive(
      join(unitDir, "launch.once"),
      "Attempt started; inspect Orca receipts before any new attempt.\n",
    );
    exclusive(
      dispatchPrompt,
      readFileSync(r.role === "author" ? this.p.authorCommon : this.p.reviewerCommon, "utf8") +
        "\n\n" +
        readFileSync(r.prompt, "utf8"),
    );
    let newShell: string | null = null;
    if (!existed) {
      const created = await this.cli(
        "worktree",
        "create",
        "--repo",
        `id:${repo.id}`,
        "--name",
        r.name,
        "--base-branch",
        r.sha,
        "--setup",
        "skip",
        "--parent-worktree",
        `path:${this.p.parent}`,
      );
      requireThat(
        created.worktree?.path === r.worktree && existsSync(r.worktree),
        "missing worktree receipt; inspect residuals",
      );
    }
    await this.clean(r.worktree, r.sha);
    await this.lineage(r.worktree);
    const terminals = (await this.cli("terminal", "list", "--worktree", `path:${r.worktree}`))
      .terminals;
    requireThat(Array.isArray(terminals), "missing terminal inventory");
    if (!existed) {
      requireThat(
        terminals.length === 1,
        "startup terminal inventory unproven; preserve checkout and inspect, never create a duplicate",
      );
      newShell = terminals[0].handle;
    }
    let handle = r.terminal;
    const argv = [
      this.p.codex,
      "-m",
      this.p.worker.model,
      "-c",
      `model_reasoning_effort=\"${this.p.worker.effort}\"`,
      "-c",
      `service_tier=\"${this.p.worker.tier}\"`,
      "--dangerously-bypass-approvals-and-sandbox",
    ];
    const command = "exec " + argv.map(quoteArg).join(" ");
    if (handle) {
      requireThat(
        existed &&
          terminals.some(
            (t) =>
              t.handle === handle &&
              t.worktreePath === r.worktree &&
              t.connected === true &&
              t.agentIdentity === "codex",
          ),
        "existing agent terminal mismatch",
      );
    } else if (newShell) {
      requireThat(
        terminals.length === 1 &&
          terminals[0].handle === newShell &&
          terminals[0].worktreePath === r.worktree &&
          terminals[0].connected === true &&
          !terminals[0].agentIdentity,
        "startup terminal is not a sole shell",
      );
      // tui-idle is an agent condition, not a plain-shell readiness signal.
      // The sole owned shell must positively expose a running prompt before exec.
      const shell = (await this.cli("terminal", "read", "--terminal", newShell)).terminal;
      requireThat(
        shell?.handle === newShell &&
          shell.status === "running" &&
          Array.isArray(shell.tail) &&
          /[$#>]\s*$/.test(shell.tail.at(-1) || "") &&
          !/Codex|Claude|grok/i.test(shell.tail.join("\n")),
        "shell readiness unproven",
      );
      const sent = (
        await this.cli("terminal", "send", "--terminal", newShell, "--text", command, "--enter")
      ).send;
      requireThat(sent?.handle === newShell && sent.accepted === true, "exec send unknown");
      handle = newShell;
    } else {
      requireThat(terminals.length === 0, "existing terminals require explicit verified reuse");
      const created = await this.cli(
        "terminal",
        "create",
        "--worktree",
        `path:${r.worktree}`,
        "--title",
        r.name,
        "--command",
        command,
      );
      handle = created.handle ?? created.terminal?.handle;
      requireThat(typeof handle === "string", "missing created terminal receipt");
    }
    const ready = (
      await this.cli(
        "terminal",
        "wait",
        "--terminal",
        handle!,
        "--for",
        "tui-idle",
        "--timeout-ms",
        "60000",
      )
    ).wait;
    requireThat(
      ready?.handle === handle &&
        ready.condition === "tui-idle" &&
        ready.satisfied === true &&
        ready.status === "running",
      "TUI readiness unknown",
    );
    const screen = (await this.cli("terminal", "read", "--terminal", handle!)).terminal;
    requireThat(screen?.handle === handle && Array.isArray(screen.tail), "missing terminal screen");
    requireThat(
      !screen.tail.join("\n").includes("Update available"),
      "update menu requires operator action; preserve resources",
    );
    assertBanner(screen.tail.join("\n"), this.p.worker);
    // Revalidate context and source immediately before the authoritative start.
    await this.context();
    await this.clean(r.worktree, r.sha);
    await this.lineage(r.worktree);
    const started = await this.cli(
      "orchestration",
      "worker-start",
      "--run",
      this.p.run,
      "--from",
      this.p.coordinator,
      "--terminal",
      handle!,
      "--worktree",
      `path:${r.worktree}`,
      "--task-title",
      `${r.role} ${r.unit}`,
      "--spec",
      r.spec,
    );
    requireThat(
      started.runId === this.p.run &&
        started.state === "ready" &&
        typeof started.taskId === "string" &&
        typeof started.dispatchId === "string" &&
        started.effects?.some(
          (e: Effect) => e.kind === "terminal" && e.role === "agent" && e.id === handle,
        ) &&
        started.effects?.some(
          (e: Effect) => e.kind === "dispatch_input" && e.id === handle && e.state === "accepted",
        ),
      "partial/non-ready worker receipt; inspect residuals, never relaunch",
    );
    return started;
  }
  async wait(out: string, envelope = false) {
    this.controllerOnly();
    requireThat(typeof out === "string", "missing output path");
    inside(this.p.scratch, out);
    requireThat(!existsSync(out), "output already exists");
    await this.context();
    return this.mailGuard(async () => {
      const result = await this.cli(
        "orchestration",
        "check",
        "--run",
        this.p.run,
        "--terminal",
        this.p.coordinator,
        "--wait",
        "--types",
        "worker_done,escalation,question",
        "--timeout-ms",
        "60000",
      );
      this.batch(result);
      exclusive(out, JSON.stringify(envelope ? { ok: true, result } : result, null, 2) + "\n");
      return result;
    });
  }
  async mailGuard<T>(action: () => Promise<T>): Promise<T> {
    requireThat(/^run_[a-z0-9]+$/.test(this.p.run), "invalid Run lock name");
    const lock = join(this.p.scratch, `waiter-${this.p.run}.lock`);
    exclusive(lock, "Single inbox operation in progress; inspect before manual recovery.\n");
    let complete = false;
    try {
      const result = await action();
      complete = true;
      return result;
    } finally {
      if (complete) unlinkSync(lock);
    }
  }
  batch(result: any) {
    requireThat(
      result.runId === this.p.run &&
        Array.isArray(result.messages) &&
        result.count === result.messages.length &&
        !result.cancelled &&
        !result.connectionLost,
      "unknown/incomplete mail batch",
    );
    requireThat(
      result.messages.length === 0 || typeof result.deliveryId === "string",
      "messages without delivery ID",
    );
  }
  async process(decisions: any) {
    this.controllerOnly();
    await this.context();
    exactKeys(decisions, ["deliveryId", "messages"]);
    return this.mailGuard(async () => {
      const result = await this.cli(
        "orchestration",
        "check",
        "--run",
        this.p.run,
        "--terminal",
        this.p.coordinator,
      );
      this.batch(result);
      requireThat(
        typeof result.deliveryId === "string" && decisions.deliveryId === result.deliveryId,
        "delivery changed/missing",
      );
      exactKeys(
        decisions.messages,
        result.messages.map((m: { id: string }) => m.id),
      );
      for (const m of result.messages) {
        const d = decisions.messages[m.id];
        if (m.type === "question") {
          exactKeys(d, ["answer"]);
          requireThat(typeof d.answer === "string" && d.answer.trim(), "missing answer");
        } else {
          exactKeys(d, ["handled"]);
          requireThat(d.handled === true, "message not handled");
        }
      }
      for (const m of result.messages)
        if (m.type === "question")
          await this.cli(
            "orchestration",
            "reply",
            "--run",
            this.p.run,
            "--id",
            m.id,
            "--body",
            decisions.messages[m.id].answer,
          );
      const next = await this.cli(
        "orchestration",
        "check",
        "--run",
        this.p.run,
        "--terminal",
        this.p.coordinator,
        "--ack",
        result.deliveryId,
      );
      this.batch(next);
      return next; // Acknowledgment may deliver new mail: never discard its result.
    });
  }
}

if (import.meta.main) {
  try {
    const args = process.argv.slice(2);
    const mode = args.shift();
    const flags: Record<string, string> = {};
    while (args.length) {
      const key = args.shift()!;
      const value = args.shift();
      requireThat(
        ["--profile", "--request", "--out", "--decisions"].includes(key) && value && !flags[key],
        "invalid arguments",
      );
      flags[key] = value;
    }
    const allowed: Record<string, string[]> = {
      inspect: ["--profile"],
      launch: ["--profile", "--request"],
      wait: ["--profile", "--out"],
      process: ["--profile", "--decisions"],
    };
    requireThat(
      mode && allowed[mode] && Object.keys(flags).every((k) => allowed[mode].includes(k)),
      "use inspect|launch --request FILE|wait --out FILE|process --decisions FILE [--profile FILE]",
    );
    const required: Record<string, string> = {
      launch: "--request",
      wait: "--out",
      process: "--decisions",
    };
    requireThat(mode === "inspect" || flags[required[mode]], "missing required argument");
    const c = new Coordinator(loadProfile(flags["--profile"]));
    const result =
      mode === "inspect"
        ? await c.inspect()
        : mode === "launch"
          ? await c.launch(privateJson(flags["--request"]))
          : mode === "wait"
            ? await c.wait(flags["--out"])
            : await c.process(privateJson(flags["--decisions"]));
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : "operation failed");
    process.exitCode = 1;
  }
}
