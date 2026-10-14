import { legacy } from "./legacy";
import { test, expect, afterEach } from "bun:test";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync,
  rmSync,
  chmodSync,
  symlinkSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  Coordinator,
  loadProfile,
  quoteArg,
  execute,
  privateJson,
  type Profile,
  type Request,
} from "./coordinator";

const roots: string[] = [];
const fixtureRoot = process.env.FVOCI_SKILL_TEST_ROOT;
if (!fixtureRoot) throw new Error("Set FVOCI_SKILL_TEST_ROOT to owned on-disk task scratch");
afterEach(() => {
  for (const r of roots.splice(0)) rmSync(r, { recursive: true, force: true });
});
const sha = "a".repeat(40);
function fixture(scenario = "good") {
  const root = mkdtempSync(join(fixtureRoot!, "case-"));
  roots.push(root);
  for (const d of ["repo", "parent", "unit", "checkout"]) mkdirSync(join(root, d));
  mkdirSync(join(root, "repo/.git"));
  for (const f of ["common.md", "review.md", "handoff.md", "state.md", "heavy", "codex", "cli"])
    writeFileSync(join(root, f), "fixture\n");
  const p: Profile = {
    version: 1,
    cli: join(root, "cli"),
    codex: join(root, "codex"),
    repo: join(root, "repo"),
    parent: join(root, "parent"),
    parentId: "repo-fixture::" + join(root, "parent"),
    parentInstance: "parent-instance",
    run: "run_fixture",
    coordinator: "term_fixture",
    handoff: join(root, "handoff.md"),
    state: join(root, "state.md"),
    scratch: root,
    heavy: join(root, "heavy"),
    authorCommon: join(root, "common.md"),
    reviewerCommon: join(root, "review.md"),
    controller: {
      model: "gpt-6.1-sol",
      effort: "xhigh",
      tier: "default",
      banner: "GPT-6.1-Sol",
    },
    worker: {
      model: "gpt-6.1-sol",
      effort: "high",
      tier: "default",
      banner: "GPT-6.1-Sol",
    },
    concurrency: 4,
  };
  process.env.ORCA_TERMINAL_HANDLE = p.coordinator;
  const prompt = join(root, "unit/prompt.md");
  writeFileSync(prompt, "Fixture ownership only");
  const request: Request = {
    role: "author",
    unit: "unit",
    branch: "draft/fixture",
    sha,
    worktree: join(root, "checkout"),
    prompt,
    report: join(root, "unit/report.md"),
    spec: `Branch draft/fixture. Target fixture. Change fixture. Constraints no remote mutations. Ownership fixture. Acceptance report. Read ${join(root, "unit/dispatch-prompt.md")}`,
    name: "fixture",
    terminal: "term_agent",
  };
  const calls = join(root, "calls.jsonl");
  const stub = join(root, "stub.ts");
  // Real Bun child CLI boundary; every invocation records argv, not a shell command.
  writeFileSync(
    stub,
    `import {appendFileSync,existsSync,mkdirSync} from 'node:fs';
const [root,scenario,...argv]=process.argv.slice(2);
appendFileSync(root+'/calls.jsonl',JSON.stringify(argv)+'\\n');
const flag=k=>argv[argv.indexOf(k)+1];
let r;
if(argv[0]==='git') { if(argv.includes('status')) console.log(scenario==='dirty'?' M fixture':''); else console.log(argv.includes('--git-common-dir')?root+'/repo/.git':scenario==='wrongsha'?'b'.repeat(40):'a'.repeat(40)); process.exit(0); }
const a=argv.slice(1), verb=a.slice(0,2).join(' ');
if(verb==='status --json') r={runtime:{reachable:true,state:'ready'}};
else if(verb==='orchestration run-show') r={run:{id:'run_fixture',coordinator_handle:scenario==='stalerun'?'term_other':'term_fixture'}};
else if(verb==='repo show') { r={repo:{id:scenario==='wrongrepo'?'other':'repo-fixture',path:root+'/repo'}}; }
else if(verb==='worktree show') {
 const path=flag('--worktree').slice(5),parent=path===root+'/parent';
 r={worktree:{id:parent?'repo-fixture::'+path:'repo-fixture::'+path,repoId:scenario==='wrongsource'&&path===root+'/repo'?'other':'repo-fixture',path,instanceId:parent?'parent-instance':'child-instance',childWorktreeIds:['repo-fixture::'+root+'/checkout','repo-fixture::'+root+'/new-checkout','repo-fixture::'+root+'/fixture','repo-fixture::'+root+'/v6-unit','repo-fixture::'+root+'/rv-grok-unit'],parentWorktreeId:scenario==='wrongparent'?'other':scenario==='missingparent'?null:'repo-fixture::'+root+'/parent',lineage:{parentWorktreeInstanceId:'parent-instance'}}};
}
else if(verb==='worktree create') { if(flag('--repo')!=='id:repo-fixture') {console.log(JSON.stringify({ok:false,error:{code:'repo_not_found'}}));process.exit(1);} const path=root+'/'+flag('--name');mkdirSync(path); r={worktree:{path}};; }
else if(verb==='terminal list') { const parent=flag('--worktree')==='path:'+root+'/parent'; r={terminals:parent?[{handle:'term_fixture',worktreePath:root+'/parent',connected:true}]:[{handle:'term_agent',connected:true,worktreePath:flag('--worktree').slice(5),agentIdentity:(['new','legacy'].includes(scenario)||(scenario==='partial'&&flag('--worktree')!=='path:'+root+'/checkout')) ? null:'codex'}]}; }
else if(verb==='terminal send') r={accepted:true};
else if(verb==='terminal wait') r={satisfied:true};
else if(verb==='terminal read') r={terminal:{handle:flag('--terminal'),status:'running',tail:flag('--terminal')==='term_fixture'?['GPT-6.1-Sol '+(scenario==='controllerwrong'?'high':'xhigh')+' · fixture']:(['new','legacy'].includes(scenario)||(scenario==='partial'&&existsSync(root+'/rv-grok-unit')))&&!existsSync(root+'/sent')?['fixture$']:['GPT-6.1-Sol high'+(scenario==='fast'?' fast':scenario==='hint'?' · ultrafast available':'')].map(s=>scenario==='wrongmodel'?s.replace('Sol','Other'):scenario==='wrongeffort'?s.replace('high','highx'):s)}};
else if(verb==='orchestration worker-start') { if(scenario==='failedstage') {console.log(JSON.stringify({ok:false,result:{failedStage:'inject',residualResources:['term_agent']}}));process.exit(1);} r=scenario==='partial'?{state:'ready',taskId:'task_fixture'}:{runId:'run_fixture',state:'ready',stage:'input_accepted',taskId:'task_fixture',dispatchId:'ctx_fixture',effects:[{kind:'terminal',role:'agent',id:'term_agent'},{kind:'dispatch_input',id:'term_agent',state:'accepted'}]}; }
else if(verb==='orchestration worker-list') { const second=a.includes('--cursor'); r={scope:{run:'run_fixture'},workers:[{dispatchId:second?'ctx_2':'ctx_1',agentTerminalHandle:scenario==='unknownagent'?null:'term_agent',dispatchStatus:scenario==='liveagent'?'dispatched':'completed',terminalState:'retained',projection:{outcome:scenario==='liveagent'?'in_progress':'succeeded',liveness:{verdict:'live'}}}],page:{hasMore:!second,nextCursor:second?null:'page2'}}; }
else if(verb==='orchestration reply') r={replied:true};
else if(verb==='orchestration check') { const ack=a.includes('--ack'); r={runId:'run_fixture',deliveryId:ack?'delivery2':'delivery1',count:ack?1:3,messages:ack?[{id:'new-arrival',type:'escalation',body:'new'}]:[{id:'heartbeat',type:'heartbeat'},{id:'question-original',type:'question',body:'answer me'},{id:'done',type:'worker_done',taskId:'task_fixture',dispatchId:'ctx_fixture'}]}; }
else throw new Error('unsupported stub call');
if(verb==='terminal send') {appendFileSync(root+'/sent','sent');}
console.log(JSON.stringify({ok:true,result:r}));
`,
  );
  const c = new Coordinator(p, (argv) =>
    execute([process.execPath, stub, root, scenario, ...argv]),
  );
  return {
    root,
    p,
    c,
    request,
    calls: () =>
      existsSync(calls)
        ? (readFileSync(calls, "utf8")
            .trim()
            .split("\n")
            .map((s) => JSON.parse(s)) as string[][])
        : [],
  };
}
test("inspect paginates read-only projections", async () => {
  const f = fixture();
  const r = await f.c.inspect();
  expect(r.workers).toHaveLength(2);
  expect(
    f
      .calls()
      .every(
        (a) =>
          a[0] === "git" ||
          [
            "status",
            "worktree show",
            "terminal list",
            "terminal read",
            "repo show",
            "git",
            "orchestration run-show",
            "orchestration worker-list",
          ].some((v) => a.slice(1).join(" ").startsWith(v)),
      ),
  ).toBe(true);
});

// Exercise the installed command, profile loading and cwd independence together.
// This fixture is a local CLI double, never a connection to a live Orca Run.
for (const scenario of ["good", "stalerun"])
  test(`installed inspect CLI from foreign cwd: ${scenario}`, async () => {
    const f = fixture(scenario);
    for (const tool of [f.p.cli, join(f.root, "git")]) {
      writeFileSync(
        tool,
        `#!${process.execPath}\nprocess.argv.splice(2, 0, ${JSON.stringify(f.root)}, ${JSON.stringify(scenario)}, ${JSON.stringify(tool === f.p.cli ? f.p.cli : "git")});\nawait import(${JSON.stringify(join(f.root, "stub.ts"))});\n`,
      );
      chmodSync(tool, 0o700);
    }
    const profile = join(f.root, "local-profile.json");
    writeFileSync(profile, JSON.stringify(f.p), { mode: 0o600 });
    const child = Bun.spawn(
      [
        process.execPath,
        fileURLToPath(new URL("./coordinator.ts", import.meta.url)),
        "inspect",
        "--profile",
        profile,
      ],
      {
        cwd: f.root,
        // No inherited locator, private scratch, or terminal binding is available.
        env: { PATH: `${f.root}:${process.env.PATH}`, HOME: f.root, XDG_STATE_HOME: f.root },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    const [stdout, stderr, exit] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);
    if (scenario === "good") {
      expect(exit).toBe(0);
      expect(stderr).toBe("");
      expect(JSON.parse(stdout).workers).toHaveLength(2);
      expect(JSON.parse(stdout).handoff).toBe(f.p.handoff);
    } else {
      expect(exit).toBe(1);
      expect(stdout).toBe("");
      expect(stderr).toContain("Run/coordinator mismatch");
      expect(f.calls()).toHaveLength(2);
    }
    expect(f.calls().some((a) => a.includes("worker-start") || a.includes("check"))).toBe(false);
  });
test("ready existing author launches once without shell input", async () => {
  const f = fixture();
  expect((await f.c.launch(f.request)).dispatchId).toBe("ctx_fixture");
  expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
  expect(f.calls().some((a) => a.includes("send") || a.includes("create"))).toBe(false);
  await expect(f.c.launch(f.request)).rejects.toThrow();
  expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
});
test("reviewer selects reviewer common prompt", async () => {
  const f = fixture();
  writeFileSync(f.p.reviewerCommon, "Reviewer-only fixture");
  await f.c.launch({ ...f.request, role: "reviewer" });
  expect(readFileSync(join(f.root, "unit/dispatch-prompt.md"), "utf8")).toContain(
    "Reviewer-only fixture",
  );
});
for (const scenario of [
  "wrongparent",
  "missingparent",
  "dirty",
  "wrongsha",
  "stalerun",
  "fast",
  "wrongmodel",
  "wrongeffort",
  "liveagent",
  "unknownagent",
])
  test(`refuses ${scenario} before dispatch`, async () => {
    const f = fixture(scenario);
    await expect(f.c.launch(f.request)).rejects.toThrow();
    expect(f.calls().some((a) => a.includes("worker-start") || a.includes("send"))).toBe(false);
  });
for (const scenario of ["failedstage", "partial"])
  test(`preserves ${scenario} without retry or cleanup`, async () => {
    const f = fixture(scenario);
    await expect(f.c.launch(f.request)).rejects.toThrow();
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
    expect(
      f
        .calls()
        .some((a) =>
          ["stop", "release", "abandon", "worker-stop", "worker-release"].includes(a[2]),
        ),
    ).toBe(false);
  });
test("full batch replies original question and exposes new ack delivery", async () => {
  const f = fixture();
  const r = await f.c.process({
    deliveryId: "delivery1",
    messages: {
      heartbeat: { handled: true },
      "question-original": { answer: "literal ` and $(false)" },
      done: { handled: true },
    },
  });
  expect(r.messages[0].id).toBe("new-arrival");
  const reply = f.calls().find((a) => a.includes("reply"))!;
  expect(reply[reply.indexOf("--id") + 1]).toBe("question-original");
  expect(reply[reply.indexOf("--body") + 1]).toBe("literal ` and $(false)");
});
test("partial decision batch never replies or acknowledges", async () => {
  const f = fixture();
  await expect(
    f.c.process({
      deliveryId: "delivery1",
      messages: { "question-original": { answer: "yes" } },
    }),
  ).rejects.toThrow();
  expect(f.calls().some((a) => a.includes("reply") || a.includes("--ack"))).toBe(false);
});
test("duplicate waiter rejected before check; guard retained", async () => {
  const f = fixture();
  const lock = join(f.root, "waiter-run_fixture.lock");
  writeFileSync(lock, "existing owner");
  await expect(f.c.wait(join(f.root, "wait.json"))).rejects.toThrow();
  expect(f.calls().some((a) => a.includes("--wait"))).toBe(false);
  expect(readFileSync(lock, "utf8")).toBe("existing owner");
});
test("delivery processor respects an existing waiter guard", async () => {
  const f = fixture();
  writeFileSync(join(f.root, "waiter-run_fixture.lock"), "existing waiter");
  await expect(f.c.process({ deliveryId: "delivery1", messages: {} })).rejects.toThrow();
  expect(f.calls().some((a) => a.includes("check"))).toBe(false);
});
test("wait returns complete batch with heartbeat and completion separate", async () => {
  const f = fixture();
  const result = await f.c.wait(join(f.root, "wait.json"));
  expect(result.messages).toHaveLength(3);
  expect(f.calls().some((a) => a.includes("--ack"))).toBe(false);
  expect(existsSync(join(f.root, "waiter-run_fixture.lock"))).toBe(false);
});
test("worker cannot consume coordinator delivery", async () => {
  const f = fixture();
  process.env.ORCA_TERMINAL_HANDLE = "term_worker";
  await expect(f.c.wait(join(f.root, "wait.json"))).rejects.toThrow("not the verified coordinator");
  expect(f.calls()).toHaveLength(0);
});
test("locator rejects broad permissions and unknown keys", () => {
  const f = fixture(),
    path = join(f.root, "profile.json");
  writeFileSync(path, JSON.stringify(f.p), { mode: 0o600 });
  expect(loadProfile(path).run).toBe("run_fixture");
  chmodSync(path, 0o644);
  expect(() => loadProfile(path)).toThrow();
  chmodSync(path, 0o600);
  writeFileSync(path, JSON.stringify({ ...f.p, token: "forbidden" }));
  expect(() => loadProfile(path)).toThrow();
});
test("command boundary single-quotes shell metacharacters", async () => {
  const f = fixture();
  const payload = "a' `false` $(false) " + f.root;
  const out = await execute(["bash", "-c", "printf '%s' " + quoteArg(payload)]);
  expect(out).toBe(payload);
});
test("real child failure and spawn failure are surfaced", async () => {
  await expect(
    execute([process.execPath, "-e", "process.stderr.write('private');process.exit(7)"]),
  ).rejects.toThrow("exit 7");
  await expect(execute([join(fixtureRoot!, "does-not-exist")])).rejects.toThrow();
});
test("new child uses explicit parent and exec in its sole startup shell", async () => {
  const f = fixture("new");
  const r = await f.c.launch({
    ...f.request,
    worktree: join(f.root, "fixture"),
    terminal: null,
  });
  expect(r.state).toBe("ready");
  const creation = f.calls().find((a) => a.includes("create"))!;
  expect(creation[creation.indexOf("--parent-worktree") + 1]).toBe(`path:${f.p.parent}`);
  expect(creation[creation.indexOf("--repo") + 1]).toBe("id:repo-fixture");
  const sends = f.calls().filter((a) => a.includes("send"));
  expect(sends).toHaveLength(1);
  expect(sends[0][sends[0].indexOf("--text") + 1]).toStartWith("exec '");
  expect(f.calls().filter((a) => a[1] === "terminal" && a[2] === "create")).toHaveLength(0);
});
test("new checkout with automatic agent never spawns a nested agent", async () => {
  const f = fixture("newagent");
  await expect(
    f.c.launch({
      ...f.request,
      worktree: join(f.root, "fixture"),
      terminal: null,
    }),
  ).rejects.toThrow("sole shell");
  expect(f.calls().some((a) => a.includes("send") || a.includes("worker-start"))).toBe(false);
});
test("report symlink parent rejected before mutation", async () => {
  const f = fixture();
  symlinkSync(join(f.root, "repo"), join(f.root, "unit/redirect"));
  await expect(
    f.c.launch({
      ...f.request,
      report: join(f.root, "unit/redirect/report.md"),
    }),
  ).rejects.toThrow("noncanonical");
  expect(f.calls()).toHaveLength(0);
});
test("actual child interruption is awaited and reported", async () => {
  await expect(
    execute([
      process.execPath,
      "-e",
      "process.kill(process.ppid,'SIGTERM');setInterval(()=>{},1000)",
    ]),
  ).rejects.toThrow("interrupted");
});

test("Fast-off hint is not an active Fast setting", async () => {
  const f = fixture("hint");
  expect((await f.c.launch(f.request)).state).toBe("ready");
});
for (const scenario of ["wrongrepo", "wrongsource", "controllerwrong"])
  test("context rejects " + scenario + " before mutation", async () => {
    const f = fixture(scenario);
    await expect(f.c.launch(f.request)).rejects.toThrow();
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(false);
    expect(f.calls().some((a) => a.includes("worker-start") || a.includes("send"))).toBe(false);
  });

function prepareLegacy(f: ReturnType<typeof fixture>) {
  mkdirSync(join(f.root, "prompts"));
  writeFileSync(
    join(f.root, "prompts/unit.md"),
    "literal unit instruction " + "$(false) and backtick " + String.fromCharCode(96) + "\n",
  );
  rmSync(join(f.root, "unit/prompt.md"));
}
for (const mode of ["worker", "review"] as const)
  test("legacy exact " + mode + " argv builds private request and dispatch", async () => {
    const f = fixture("legacy");
    prepareLegacy(f);
    const report = join(f.root, "unit/report.md");
    const argv =
      mode === "worker"
        ? [mode, "unit", "draft/fixture", sha]
        : [mode, "unit", sha, report, "draft/fixture"];
    const previous = process.cwd();
    process.chdir(f.root);
    try {
      expect((await legacy(argv, f.p, f.c)).dispatchId).toBe("ctx_fixture");
    } finally {
      process.chdir(previous);
    }
    const req = JSON.parse(readFileSync(join(f.root, "unit/request.json"), "utf8"));
    expect(req.worktree).toBe(join(f.root, mode === "worker" ? "v6-unit" : "rv-grok-unit"));
    expect(req.report).toBe(report);
    expect(req.branch).toBe("draft/fixture");
    expect(req.sha).toBe(sha);
    expect(req.terminal).toBeNull();
    expect(req.role).toBe(mode === "worker" ? "author" : "reviewer");
    expect(privateJson(join(f.root, "unit/request.json"))).toEqual(req);
    const combined = readFileSync(join(f.root, "unit/dispatch-prompt.md"), "utf8");
    expect(combined).toContain(
      readFileSync(mode === "worker" ? f.p.authorCommon : f.p.reviewerCommon, "utf8"),
    );
    expect(combined).toContain("$(false)");
    const start = f.calls().find((a) => a.includes("worker-start"))!;
    expect(start[start.indexOf("--spec") + 1]).toBe(req.spec);
    expect(start[start.indexOf("--from") + 1]).toBe(f.p.coordinator);
    expect(start[start.indexOf("--run") + 1]).toBe(f.p.run);
    const create = f.calls().find((a) => a[1] === "worktree" && a[2] === "create")!;
    expect(create[create.indexOf("--repo") + 1]).toBe("id:repo-fixture");
    expect(create[create.indexOf("--parent-worktree") + 1]).toBe("path:" + f.p.parent);
    const send = f.calls().find((a) => a.includes("send"))!;
    const command = send[send.indexOf("--text") + 1];
    expect(command).toContain('model_reasoning_effort="high"');
    expect(command).toContain('service_tier="default"');
    expect(
      f.calls().some((a) => a.includes("fetch") || a.includes("set") || a.includes("--ack")),
    ).toBe(false);
    expect(
      JSON.parse(readFileSync(join(f.root, "unit/start.json"), "utf8")).result.dispatchId,
    ).toBe("ctx_fixture");
  });
test("legacy exact wait argv writes complete compatible envelope once", async () => {
  const f = fixture();
  const out = join(f.root, "receipt.json");
  const result = await legacy(["wait", out], f.p, f.c);
  expect(JSON.parse(readFileSync(out, "utf8"))).toEqual({ ok: true, result });
  expect(result.messages).toHaveLength(3);
  expect(f.calls().filter((a) => a.includes("--wait"))).toHaveLength(1);
  expect(f.calls().some((a) => a.includes("--ack"))).toBe(false);
});
for (const kind of [
  "branch",
  "sha",
  "report",
  "prompt",
  "common",
  "effort",
  "model",
  "tier",
  "active",
  "parent",
  "partial",
])
  test("legacy rejects " + kind + " and preserves failure", async () => {
    const f = fixture(
      kind === "parent" ? "wrongparent" : kind === "partial" ? "partial" : "legacy",
    );
    prepareLegacy(f);
    let args = ["review", "unit", sha, join(f.root, "unit/report.md"), "draft/fixture"];
    if (kind === "branch") args[4] = "main";
    if (kind === "sha") args[2] = "main";
    if (kind === "report") args[3] = "relative.md";
    if (kind === "prompt") rmSync(join(f.root, "prompts/unit.md"));
    if (kind === "common") rmSync(f.p.reviewerCommon);
    if (kind === "effort") process.env.SOL_EFFORT = "xhigh";
    if (kind === "model") f.p.worker.model = "gpt-6-astra";
    if (kind === "tier") f.p.worker.tier = "fast";
    if (kind === "active" || kind === "parent") mkdirSync(join(f.root, "rv-grok-unit"));
    try {
      await expect(legacy(args, f.p, f.c)).rejects.toThrow();
    } finally {
      delete process.env.SOL_EFFORT;
    }
    expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(
      kind === "partial" ? 1 : 0,
    );
    if (kind === "partial") expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    else expect(f.calls().some((a) => a.includes("send") || a[2] === "create")).toBe(false);
  });
test("legacy preserves existing waiter and receipt", async () => {
  const f = fixture();
  const lock = join(f.root, "waiter-run_fixture.lock");
  writeFileSync(lock, "active owner");
  await expect(legacy(["wait", join(f.root, "new-receipt.json")], f.p, f.c)).rejects.toThrow();
  expect(readFileSync(lock, "utf8")).toBe("active owner");
  expect(f.calls().some((a) => a.includes("check"))).toBe(false);
});

test("legacy reviewer report metacharacters remain literal argv", async () => {
  const f = fixture("legacy");
  prepareLegacy(f);
  const report = join(f.root, "unit/report ' $(false) `false`.md");
  await legacy(["review", "unit", sha, report, "draft/fixture"], f.p, f.c);
  const req = privateJson(join(f.root, "unit/request.json"));
  expect(req.report).toBe(report);
  const call = f.calls().find((a) => a.includes("worker-start"))!;
  expect(call[call.indexOf("--spec") + 1]).toBe(req.spec);
  expect(existsSync(report)).toBe(false);
});
test("legacy refuses an existing report without dispatch", async () => {
  const f = fixture("legacy");
  prepareLegacy(f);
  const report = join(f.root, "unit/report.md");
  writeFileSync(report, "preserved");
  await expect(
    legacy(["review", "unit", sha, report, "draft/fixture"], f.p, f.c),
  ).rejects.toThrow();
  expect(readFileSync(report, "utf8")).toBe("preserved");
  expect(f.calls().some((a) => a.includes("worker-start") || a.includes("send"))).toBe(false);
});
