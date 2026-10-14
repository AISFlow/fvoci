import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import {
  Coordinator,
  checkModel,
  launchEntry,
  loadProfile,
  exclusive,
  inside,
  ownedDirectory,
  privateJson,
  requireThat,
  type Profile,
  type Request,
} from "./coordinator";

// Mechanical adapter for the three existing entrypoints. Lifecycle stays in Coordinator.
export async function legacy(args: string[], p: Profile, c = new Coordinator(p)) {
  const [mode, ...values] = args;
  requireThat(
    (mode === "worker" && values.length === 3) ||
      (mode === "review" && values.length === 4) ||
      (mode === "wait" && values.length === 1),
    "invalid legacy argv",
  );
  c.controllerOnly();
  if (mode === "wait") return c.wait(values[0], true);
  const reviewer = mode === "review";
  const entry = launchEntry(p, reviewer ? "reviewer" : "author");
  // The selected adapter must be supported and satisfy its own constraints before any mutation.
  checkModel(entry);
  requireThat(
    (process.env.SOL_EFFORT ?? entry.effort) === entry.effort,
    "legacy effort override differs from the profile",
  );
  const [unit, second, third, fourth] = values;
  const sha = reviewer ? second : third;
  const branch = reviewer ? fourth : second;
  requireThat(
    /^[a-z0-9][a-z0-9-]*$/.test(unit) &&
      /^[a-f0-9]{40}$/.test(sha) &&
      /^draft\/[a-zA-Z0-9/_-]+$/.test(branch),
    "require valid unit, draft branch and SHA40",
  );
  const unitDir = join(p.scratch, unit);
  const report = reviewer ? third : join(unitDir, "report.md");
  requireThat(isAbsolute(report), "report must be absolute");
  const sourcePrompt = join(p.scratch, "prompts", unit + ".md");
  requireThat(
    existsSync(sourcePrompt) && existsSync(reviewer ? p.reviewerCommon : p.authorCommon),
    "missing prompt/common",
  );
  // Codex reviewers keep the existing compatibility name. Claude reviewers get their own prefix so a
  // leftover checkout from the other adapter is never adopted as this attempt's checkout.
  const name = (reviewer ? (entry.adapter === "claude" ? "rv-claude-" : "rv-grok-") : "v6-") + unit;
  const worktree = join(dirname(p.parent), name);
  const prompt = join(unitDir, "prompt.md");
  const requestPath = join(unitDir, "request.json");
  const dispatchPrompt = join(unitDir, "dispatch-prompt.md");
  // Validate runtime/source before creating request artifacts; never fetch or pick a terminal.
  await c.context();
  await c.clean(p.repo);
  requireThat(
    (await c.exec(["git", "-C", p.repo, "rev-parse", `${sha}^{commit}`])).trim() === sha,
    "source commit missing",
  );
  if (!existsSync(unitDir)) mkdirSync(unitDir, { mode: 0o700 });
  ownedDirectory(unitDir);
  inside(unitDir, report);
  requireThat(
    ![report, requestPath, dispatchPrompt, join(unitDir, "launch.once")].some(existsSync),
    "existing report/request/attempt; inspect previous work",
  );
  if (existsSync(worktree)) {
    ownedDirectory(worktree);
    await c.clean(worktree, sha);
    await c.lineage(worktree);
    const inventory = (await c.cli("terminal", "list", "--worktree", `path:${worktree}`)).terminals;
    requireThat(
      Array.isArray(inventory) && inventory.length === 0,
      "existing terminals require explicit verified reuse; legacy entrypoint never selects one",
    );
  }
  const instructions = readFileSync(sourcePrompt, "utf8");
  // An existing unit prompt may be retained only when it is byte-identical to the source.
  const unitPrompt =
    (reviewer
      ? `Your review worktree is ${worktree}, a checkout of the reviewed head (its local branch name is Orca's; never push it). Durable scratch dir: ${unitDir}\n\n`
      : "") + instructions;
  if (existsSync(prompt))
    requireThat(
      readFileSync(prompt, "utf8") === unitPrompt,
      "existing prompt differs; preserve it",
    );
  else exclusive(prompt, unitPrompt);
  const spec = reviewer
    ? `Independent read-only code review (${unit}). Target: AISFlow/fvoci branch ${branch} at ${sha}, checked out in this worktree. Change: produce a review report with verdict ACCEPT or REQUEST_CHANGES. Full instructions are in ${dispatchPrompt}; read them first. Constraints: do not edit, commit or push the reviewed branch; run every heavy command through ${p.heavy}. Ownership: only scratch ${unitDir} and report ${report}. Observable acceptance: report ${report} with verdict; send worker_done with --report-path ${report}.`
    : `Work unit ${unit} on AISFlow/fvoci branch ${branch} (start at ${sha}), checked out in this worktree. Change and ownership: follow full instructions in ${dispatchPrompt}; read them first, including ${p.heavy} resource rules. Constraints: no PR, no force push, push only HEAD:${branch} fast-forward when unit instructions authorize Git submission; no new Python or shell harness. Ownership: only files named in that prompt and scratch ${unitDir}. Observable acceptance: complete the unit-specific acceptance and report ${report} ending with PR: none; send worker_done with --report-path ${report}. Unit-specific no-Git instructions take precedence.`;
  const request: Request = {
    role: reviewer ? "reviewer" : "author",
    unit,
    branch,
    sha,
    worktree,
    prompt,
    report,
    spec,
    name,
    terminal: null,
  };
  exclusive(requestPath, JSON.stringify(request, null, 2) + "\n");
  const started = await c.launch(privateJson(requestPath));
  exclusive(
    join(unitDir, "start.json"),
    JSON.stringify({ ok: true, result: started }, null, 2) + "\n",
  );
  return started;
}
if (import.meta.main) {
  try {
    const args = process.argv.slice(2);
    let profile: string | undefined;
    if (args.at(-2) === "--profile") {
      profile = args.pop();
      args.pop();
    }
    console.log(JSON.stringify(await legacy(args, loadProfile(profile)), null, 2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : "legacy operation failed");
    process.exitCode = 1;
  }
}
