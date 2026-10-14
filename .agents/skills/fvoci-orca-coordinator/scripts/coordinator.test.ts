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
// Claude transcript directories are mapped only for path characters observed on the installed CLI.
if (!/^[A-Za-z0-9/-]+$/.test(fixtureRoot))
  throw new Error("FVOCI_SKILL_TEST_ROOT must use only letters, digits, '/' and '-'");
afterEach(() => {
  for (const r of roots.splice(0)) rmSync(r, { recursive: true, force: true });
});
const sha = "a".repeat(40);
// Sanitized real `terminal read` tails from installed agent TUIs and plain shells on an Orca host.
// User, host and paths are replaced; glyphs, hint text and line layout are kept as captured.
const rule = (n: number) => "─".repeat(n);
const realScreens: Record<string, string[]> = {
  "claude screen": [
    "  ⎿  Shell cwd was reset to /work/checkout",
    "✽ Doodling… (4m 2s · ↓ 8.9k tokens · thought for 4s)",
    "  ⎿  Tip: Use /btw to ask a quick side question without interrupting Claude's current work",
    rule(120),
    "❯",
    rule(120),
    "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents",
  ],
  "claude stream": ["✶ Doodling…se4/b2s · ↓ 8.9k tokens · thought for 4s)", "", "", "", ""],
  "claude working": [
    "✢─Working… ────59s · ↓ 16.1k tokens · deep in thought with xhigh effort)",
    "",
    "",
  ],
  "codex screen": [
    "• Ran /work/bin/bun run - <<'JS' …",
    "  └ (no output)",
    "  Worked for 11m 11s • 23:07",
    "› Ask Codex to do anything",
    "  GPT-6-Astra high · ~/work/checkout · task title",
    "  ? for shortcuts",
  ],
  "grok screen": [
    "     Worked for 8.5s",
    "  ┃  ◆ Recap",
    "  ╭" + rule(60) + "╮",
    "  │ ❯",
    "  ╰" + rule(40) + " Weekly limit left: 2% · Grok 4.7 Fast (xhigh) · always-approve ─╯",
    "  Shift+Tab:mode  │  Ctrl+x:shortcuts",
  ],
};
const realShells: Record<string, string[]> = {
  "idle shell": ["", "fixture@fixture:~/work/fvoci$"],
  "shell after output": [
    "fixture@fixture:~/work/other$ orca skills update --all",
    "Checking skills from source: stablyai/orca",
    "✓ All global skills are up to date",
    "",
    "",
    "fixture@fixture:~/work/other$",
  ],
};
const claudeOpus = {
  adapter: "claude" as const,
  model: "claude-opus-5-5",
  effort: "high",
  tier: "default",
  banner: "Opus 5.5",
};
function fixture(scenario = "good", shellTail?: string[]) {
  const root = mkdtempSync(join(fixtureRoot!, "case-"));
  roots.push(root);
  for (const d of ["repo", "parent", "unit", "checkout"]) mkdirSync(join(root, d));
  mkdirSync(join(root, "repo/.git"));
  for (const f of ["common.md", "review.md", "handoff.md", "state.md", "heavy", "codex", "cli"])
    writeFileSync(join(root, f), "fixture\n");
  const kind = /^(claude|claudectl)-/.exec(scenario)?.[1];
  const p: Profile = {
    version: 2,
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
    controller:
      kind === "claudectl"
        ? { ...claudeOpus, effort: "xhigh" }
        : {
            adapter: "codex",
            model: "gpt-6.1-sol",
            effort: "xhigh",
            tier: "default",
            banner: "GPT-6.1-Sol",
          },
    worker: kind
      ? { ...claudeOpus }
      : {
          adapter: "codex",
          model: "gpt-6.1-sol",
          effort: "high",
          tier: "default",
          banner: "GPT-6.1-Sol",
        },
    concurrency: 4,
  };
  process.env.ORCA_TERMINAL_HANDLE = p.coordinator;
  // The test runner may itself run inside a Claude Code session; never read its transcript.
  delete process.env.CLAUDE_CODE_SESSION_ID;
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
    `import {appendFileSync,existsSync,mkdirSync,readFileSync} from 'node:fs';
const [root,raw,...argv]=process.argv.slice(2);
const kind=/^(claude|claudectl)-/.exec(raw)?.[1]??'',scenario=kind?raw.slice(kind.length+1):raw;
const shellTail=${JSON.stringify(shellTail ?? null)},codexScreen=${JSON.stringify(realScreens["codex screen"])},claudeIdle=${JSON.stringify(realScreens["claude screen"])};
appendFileSync(root+'/calls.jsonl',JSON.stringify(argv)+'\\n');
const flag=k=>argv[argv.indexOf(k)+1];
let r;
if(argv[0]==='git') { if(argv.includes('status')) console.log(scenario==='dirty'?' M fixture':''); else console.log(argv.includes('--git-common-dir')?root+'/repo/.git':scenario==='wrongsha'?'b'.repeat(40):'a'.repeat(40)); process.exit(0); }
const a=argv.slice(1), verb=a.slice(0,2).join(' ');
const shell=['new','legacy'].includes(scenario)||scenario.startsWith('shell-')||scenario.startsWith('new-')||(scenario==='partial'&&existsSync(root+'/created'));
if(verb==='status --json') r={runtime:{reachable:true,state:'ready'}};
else if(verb==='orchestration run-show') r={run:{id:'run_fixture',coordinator_handle:scenario==='stalerun'?'term_other':'term_fixture'}};
else if(verb==='repo show') { r={repo:{id:scenario==='wrongrepo'?'other':'repo-fixture',path:root+'/repo'}}; }
else if(verb==='worktree show') {
 const path=flag('--worktree').slice(5),parent=path===root+'/parent';
 r={worktree:{id:'repo-fixture::'+path,repoId:scenario==='wrongsource'&&path===root+'/repo'?'other':'repo-fixture',path,instanceId:parent?(scenario==='shell-stale-parent'&&existsSync(root+'/created')?'other':'parent-instance'):'child-instance',childWorktreeIds:scenario==='reverseparent'?[]:['repo-fixture::'+root+'/checkout','repo-fixture::'+root+'/new-checkout','repo-fixture::'+root+'/fixture','repo-fixture::'+root+'/v6-unit','repo-fixture::'+root+'/rv-grok-unit',...(existsSync(root+'/created')?['repo-fixture::'+readFileSync(root+'/created','utf8')]:[])],parentWorktreeId:scenario==='wrongparent'||(scenario==='shell-wrong-parent'&&!parent)?'other':scenario==='missingparent'?null:'repo-fixture::'+root+'/parent',lineage:{parentWorktreeInstanceId:scenario==='wronginstance'?'other':'parent-instance'}}};
}
else if(verb==='worktree create') { if(flag('--repo')!=='id:repo-fixture') {console.log(JSON.stringify({ok:false,error:{code:'repo_not_found'}}));process.exit(1);} const path=root+'/'+flag('--name');mkdirSync(path);appendFileSync(root+'/created',path); r={worktree:{path},startupTerminal:null,warnings:[]}; }
else if(verb==='terminal list') { const parent=flag('--worktree')==='path:'+root+'/parent'; const t={handle:'term_agent',connected:scenario!=='shell-disconnected',worktreePath:['shell-foreign','foreignterminal'].includes(scenario)?root+'/foreign':flag('--worktree').slice(5),agentIdentity:scenario==='shell-unknown-agent'?'unknown':shell?null:'codex'}; r={terminals:parent?[{handle:'term_fixture',worktreePath:root+'/parent',connected:true,...(kind==='claudectl'?{agentIdentity:scenario==='ctl-codex'?'codex':'claude'}:{})}]:scenario==='shell-missing'?[]:scenario==='shell-multiple'?[t,{...t,handle:'term_other'}]:kind?(shell?[{...t,handle:'term_startup'}]:['agent','newagent'].includes(scenario)?[{...t,handle:'term_auto',agentIdentity:'claude'}]:[]):[t]}; }
else if(verb==='terminal send') r=scenario==='shell-send-flat'?{accepted:true}:scenario==='shell-send-missing'?{}:{send:{handle:scenario==='shell-send-wrong'?'term_other':flag('--terminal'),accepted:scenario!=='shell-send-false',bytesWritten:42,prompt:null}};
else if(verb==='terminal wait') {
 if(shell&&!existsSync(root+'/sent')) {console.log(JSON.stringify({ok:false,error:{code:'timeout'}}));process.exit(1);}
 r=scenario==='wait-flat'?{satisfied:true}:scenario==='wait-missing'?{}:{wait:{handle:scenario==='wait-wrong'?'term_other':flag('--terminal'),condition:scenario==='wait-condition'?'exit':'tui-idle',satisfied:scenario!=='wait-false',status:scenario==='wait-stopped'?'exited':'running',exitCode:null}};
}
else if(verb==='terminal read') { const parent=flag('--terminal')==='term_fixture',before=shell&&!existsSync(root+'/sent'); r={terminal:{handle:!parent&&scenario==='shell-read-wrong'?'term_other':flag('--terminal'),status:!parent&&scenario==='shell-stopped'?'exited':'running',tail:parent?(kind==='claudectl'?(scenario==='screen'?['  Opus 5.5 · xhigh']:scenario==='screen-prose'?['• Claude Opus 5.5 (xhigh) is the selected controller',...claudeIdle]:claudeIdle):['GPT-6.1-Sol '+(scenario==='controllerwrong'?'high':'xhigh')+' · fixture']):before?(shellTail??(scenario==='shell-no-prompt'?['starting']:scenario==='shell-agent-screen'?[...codexScreen,'fixture$']:['fixture@fixture:'+readFileSync(root+'/created','utf8')+'$ '])):['GPT-6.1-Sol high'+(scenario==='fast'?' fast':scenario==='hint'?' · ultrafast available':'')].map(s=>scenario==='wrongmodel'?s.replace('Sol','Other'):scenario==='wrongeffort'?s.replace('high','highx'):s)}}; }
else if(verb==='orchestration worker-start'&&argv.includes('--agent')) {
 const s=scenario.replace(/^new-/,''),req={agent:flag('--agent'),model:flag('--model'),effort:flag('--effort')};
 if(s==='nonready') {console.log(JSON.stringify({ok:false,result:{state:'failed',failedStage:'agent_ready',residualResources:[{kind:'terminal',role:'agent',id:'term_claude'}]}}));process.exit(1);}
 const agent=s==='reuseshell'?'term_startup':'term_claude';
 r=s==='partial'?{runId:'run_fixture',state:'ready',taskId:'task_fixture'}:{runId:s==='wrongrun'?'run_other':'run_fixture',state:'ready',stage:'input_accepted',taskId:'task_fixture',dispatchId:'ctx_fixture',launch:{requested:req,effective:s==='effective'?{...req,model:'claude-fable-5-1'}:s==='effort'?{...req,effort:'max'}:req},effects:[{kind:'worktree',action:'reused',id:'repo-fixture::'+flag('--worktree').slice(5)},{kind:'terminal',role:'agent',action:'created',id:agent},{kind:'dispatch_input',role:'agent',id:agent,state:'accepted'}],residualResources:[]};
}
else if(verb==='orchestration worker-show') { const id=flag('--dispatch'); r={dispatch:{id,runId:scenario==='wrongrun'?'run_other':'run_fixture'},worker:{dispatchId:id,worktreeId:'repo-fixture::/work/fvoci/checkout',startOptions:{launch:{effective:{agent:'claude',model:scenario==='effective'?'claude-fable-5-1':'claude-opus-5-5',effort:'high'}}}}}; }
else if(verb==='orchestration worker-start') { if(scenario==='failedstage') {console.log(JSON.stringify({ok:false,result:{failedStage:'inject',residualResources:['term_agent']}}));process.exit(1);} r=scenario==='partial'?{state:'ready',taskId:'task_fixture'}:{runId:'run_fixture',state:'ready',stage:'input_accepted',taskId:'task_fixture',dispatchId:'ctx_fixture',effects:[{kind:'terminal',role:'agent',id:'term_agent'},{kind:'dispatch_input',id:'term_agent',state:'accepted'}]}; }
else if(verb==='orchestration worker-list') { const second=a.includes('--cursor'); r={scope:{run:'run_fixture'},workers:[{dispatchId:second?'ctx_2':'ctx_1',agentTerminalHandle:scenario==='unknownagent'?null:'term_agent',dispatchStatus:scenario==='liveagent'?'dispatched':'completed',terminalState:'retained',projection:{outcome:scenario==='liveagent'?'in_progress':'succeeded',liveness:{verdict:'live'}}}],page:{hasMore:!second,nextCursor:second?null:'page2'}}; }
else if(verb==='orchestration reply') r={replied:true};
else if(verb==='orchestration check') { const ack=a.includes('--ack'); r={runId:'run_fixture',deliveryId:ack?'delivery2':'delivery1',count:ack?1:3,messages:ack?[{id:'new-arrival',type:'escalation',body:'new'}]:[{id:'heartbeat',type:'heartbeat'},{id:'question-original',type:'question',body:'answer me'},{id:'done',type:'worker_done',taskId:'task_fixture',dispatchId:'ctx_fixture'}]}; }
else throw new Error('unsupported stub call');
if(verb==='terminal send') {appendFileSync(root+'/sent','sent');}
console.log(JSON.stringify({ok:true,result:r}));
`,
  );
  const c = new Coordinator(
    p,
    (argv) => execute([process.execPath, stub, root, scenario, ...argv]),
    join(root, "claude-projects"),
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
  const calls = f.calls();
  const sent = calls.indexOf(calls.find((a) => a.includes("send"))!);
  const waits = calls.filter((a) => a[1] === "terminal" && a[2] === "wait");
  expect(waits).toHaveLength(1);
  expect(calls.indexOf(waits[0])).toBeGreaterThan(sent);
  const created = calls.indexOf(calls.find((a) => a[1] === "worktree" && a[2] === "create")!);
  expect(
    calls.slice(0, created).some((a) => a.includes("show") && a.includes(`path:${f.p.parent}`)),
  ).toBe(true);
  expect(
    calls
      .slice(created + 1, sent)
      .some((a) => a.includes("show") && a.includes(`path:${join(f.root, "fixture")}`)),
  ).toBe(true);
  expect(calls.some((a) => a.includes("--no-parent"))).toBe(false);
});

for (const scenario of [
  "wait-flat",
  "wait-missing",
  "wait-wrong",
  "wait-condition",
  "wait-false",
  "wait-stopped",
  "foreignterminal",
])
  test(`refuses ${scenario} receipt/ownership without dispatch or cleanup`, async () => {
    const f = fixture(scenario);
    await expect(f.c.launch(f.request)).rejects.toThrow();
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(f.calls().some((a) => a.includes("worker-start") || a.includes("send"))).toBe(false);
    expect(f.calls().some((a) => ["release", "stop", "close", "abandon"].includes(a[2]))).toBe(
      false,
    );
  });

for (const scenario of [
  "shell-missing",
  "shell-multiple",
  "shell-foreign",
  "shell-disconnected",
  "shell-no-prompt",
  "shell-read-wrong",
  "shell-stopped",
  "shell-agent-screen",
  "shell-unknown-agent",
  "shell-wrong-parent",
  "shell-stale-parent",
  "shell-send-flat",
  "shell-send-missing",
  "shell-send-wrong",
  "shell-send-false",
])
  test(`preserves new checkout on ${scenario} without retry`, async () => {
    const f = fixture(scenario);
    const worktree = join(f.root, "fixture");
    await expect(f.c.launch({ ...f.request, worktree, terminal: null })).rejects.toThrow();
    expect(existsSync(worktree)).toBe(true);
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(existsSync(join(f.root, "unit/dispatch-prompt.md"))).toBe(true);
    expect(f.calls().filter((a) => a.includes("send"))).toHaveLength(
      scenario.startsWith("shell-send-") ? 1 : 0,
    );
    expect(f.calls().some((a) => a.includes("worker-start"))).toBe(false);
    expect(f.calls().filter((a) => a[1] === "worktree" && a[2] === "create")).toHaveLength(1);
    expect(
      f.calls().some((a) => a[1] === "terminal" && ["create", "close", "stop"].includes(a[2])),
    ).toBe(false);
  });

for (const name of [
  "v6-unit",
  "rv-grok-unit",
  "v6-codex-claude-grok-unit",
  "rv-grok-4-claude-code-unit",
])
  test(`owned PS1 cwd permits one exec: ${name}`, async () => {
    const f = fixture("new");
    expect(
      (await f.c.launch({ ...f.request, name, worktree: join(f.root, name), terminal: null }))
        .dispatchId,
    ).toBe("ctx_fixture");
    expect(f.calls().filter((a) => a.includes("send"))).toHaveLength(1);
    expect(f.calls().filter((a) => a.includes("wait"))).toHaveLength(1);
    expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
    await expect(
      f.c.launch({ ...f.request, name, worktree: join(f.root, name), terminal: null }),
    ).rejects.toThrow();
    expect(f.calls().filter((a) => a.includes("send"))).toHaveLength(1);
  });

for (const [name, tail] of Object.entries(realShells))
  test(`real plain-shell capture permits one exec: ${name}`, async () => {
    const f = fixture("new", tail);
    expect(
      (await f.c.launch({ ...f.request, worktree: join(f.root, "fixture"), terminal: null }))
        .dispatchId,
    ).toBe("ctx_fixture");
    expect(f.calls().filter((a) => a.includes("send"))).toHaveLength(1);
  });
// Real agent screens with an appended prompt line and no structured identity must still refuse.
for (const [name, tail] of Object.entries(realScreens))
  test(`real agent screen refuses exec despite a prompt: ${name}`, async () => {
    const f = fixture("new", [...tail, "fixture@fixture:/work/rv-grok-unit$ "]);
    await expect(
      f.c.launch({ ...f.request, worktree: join(f.root, "fixture"), terminal: null }),
    ).rejects.toThrow("shell readiness unproven");
    expect(f.calls().some((a) => a.includes("send") || a.includes("worker-start"))).toBe(false);
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(existsSync(join(f.root, "fixture"))).toBe(true);
  });

for (const scenario of ["wronginstance", "reverseparent"])
  test(`refuses ${scenario} lineage before mutation`, async () => {
    const f = fixture(scenario);
    await expect(f.c.launch(f.request)).rejects.toThrow();
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(false);
    expect(f.calls().some((a) => a.includes("worker-start") || a.includes("send"))).toBe(false);
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

// Claude Code execution adapter: Orca's own claude launcher through one worker-start call.
const uuid = (n: number) => `${String(n).padStart(8, "0")}-0000-4000-8000-000000000000`;
function transcript(f: ReturnType<typeof fixture>, cwd: string, n: number, records: object[]) {
  const dir = join(f.root, "claude-projects", cwd.replaceAll("/", "-"));
  mkdirSync(dir, { recursive: true });
  const file = join(dir, uuid(n) + ".jsonl");
  writeFileSync(file, records.map((r) => JSON.stringify(r)).join("\n") + "\n", { mode: 0o600 });
  chmodSync(file, 0o600);
  return dir;
}
// Record shapes follow the installed Claude Code transcript: launch cwd, string user prompt, and
// assistant turns carrying message.model and effort.
const injected = (cwd: string, id = "ctx_fixture") => ({
  type: "user",
  cwd,
  message: { role: "user", content: `Your task ... --dispatch-id ${id} --outcome succeeded` },
});
const turn = (cwd: string, model = "claude-opus-5-5", effort = "high") => ({
  type: "assistant",
  cwd,
  effort,
  message: { model, role: "assistant" },
});
const workerPath = "/work/fvoci/checkout";
const mutations = (f: ReturnType<typeof fixture>) =>
  f
    .calls()
    .filter(
      (a) =>
        ["send", "wait", "close", "stop", "release", "abandon"].includes(a[2]) ||
        (a[1] === "terminal" && a[2] === "create"),
    );

test("claude existing checkout starts once with requested agent, model and effort", async () => {
  const f = fixture("claude-good");
  const r = await f.c.launch({ ...f.request, terminal: null });
  expect(r.dispatchId).toBe("ctx_fixture");
  expect(r.preservedTerminals).toEqual([]);
  const starts = f.calls().filter((a) => a.includes("worker-start"));
  expect(starts).toHaveLength(1);
  const s = starts[0];
  const at = (k: string) => s[s.indexOf(k) + 1];
  expect([at("--agent"), at("--model"), at("--effort")]).toEqual([
    "claude",
    "claude-opus-5-5",
    "high",
  ]);
  expect([at("--worktree"), at("--run"), at("--from")]).toEqual([
    `path:${f.request.worktree}`,
    f.p.run,
    f.p.coordinator,
  ]);
  expect(s.includes("--terminal")).toBe(false);
  expect(mutations(f)).toHaveLength(0);
  expect(f.calls().some((a) => a[1] === "worktree" && a[2] === "create")).toBe(false);
  await expect(f.c.launch({ ...f.request, terminal: null })).rejects.toThrow();
  expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
});
test("claude new child preserves its startup shell untouched", async () => {
  const f = fixture("claude-new");
  const worktree = join(f.root, "fixture");
  const r = await f.c.launch({ ...f.request, worktree, terminal: null });
  expect(r.preservedTerminals).toEqual(["term_startup"]);
  expect(
    JSON.parse(readFileSync(join(f.root, "unit/startup-terminals.json"), "utf8")).preserved,
  ).toEqual(["term_startup"]);
  const calls = f.calls();
  const created = calls.findIndex((a) => a[1] === "worktree" && a[2] === "create");
  const started = calls.findIndex((a) => a.includes("worker-start"));
  expect(calls[created][calls[created].indexOf("--parent-worktree") + 1]).toBe(
    `path:${f.p.parent}`,
  );
  // Both lineage directions are rechecked between creation and the single start.
  const between = calls.slice(created + 1, started).map((a) => a.join(" "));
  expect(between.some((a) => a.includes(`show --worktree path:${worktree}`))).toBe(true);
  expect(between.some((a) => a.includes(`show --worktree path:${f.p.parent}`))).toBe(true);
  expect(mutations(f)).toHaveLength(0);
  expect(calls.some((a) => a.includes("term_startup"))).toBe(false);
});
for (const scenario of [
  "new-effective",
  "new-effort",
  "new-wrongrun",
  "new-partial",
  "new-nonready",
  "new-reuseshell",
])
  test(`claude refuses ${scenario} receipt and preserves residuals`, async () => {
    const f = fixture("claude-" + scenario);
    const worktree = join(f.root, "fixture");
    await expect(f.c.launch({ ...f.request, worktree, terminal: null })).rejects.toThrow(
      "term_startup",
    );
    expect(existsSync(worktree)).toBe(true);
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(existsSync(join(f.root, "unit/startup-terminals.json"))).toBe(true);
    expect(f.calls().filter((a) => a.includes("worker-start"))).toHaveLength(1);
    expect(f.calls().filter((a) => a[1] === "worktree" && a[2] === "create")).toHaveLength(1);
    expect(mutations(f)).toHaveLength(0);
  });
for (const scenario of [
  "wrongparent",
  "missingparent",
  "wronginstance",
  "reverseparent",
  "agent",
  "dirty",
  "wrongsha",
  "stalerun",
])
  test(`claude refuses ${scenario} before worker-start`, async () => {
    const f = fixture("claude-" + scenario);
    await expect(f.c.launch({ ...f.request, terminal: null })).rejects.toThrow();
    expect(f.calls().some((a) => a.includes("worker-start"))).toBe(false);
    expect(mutations(f)).toHaveLength(0);
  });
for (const scenario of [
  "shell-wrong-parent",
  "shell-stale-parent",
  "shell-foreign",
  "shell-unknown-agent",
  "newagent",
])
  test(`claude new checkout refuses ${scenario} without start or cleanup`, async () => {
    const f = fixture("claude-" + scenario);
    const worktree = join(f.root, "fixture");
    await expect(f.c.launch({ ...f.request, worktree, terminal: null })).rejects.toThrow();
    expect(existsSync(worktree)).toBe(true);
    expect(existsSync(join(f.root, "unit/launch.once"))).toBe(true);
    expect(f.calls().some((a) => a.includes("worker-start"))).toBe(false);
    expect(mutations(f)).toHaveLength(0);
  });
test("claude refuses explicit terminal reuse before any call", async () => {
  const f = fixture("claude-good");
  await expect(f.c.launch(f.request)).rejects.toThrow("terminal reuse");
  expect(f.calls()).toHaveLength(0);
});
test("unknown adapter is refused before any call", async () => {
  const f = fixture();
  (f.p.worker as any).adapter = "grok";
  await expect(f.c.launch({ ...f.request, terminal: null })).rejects.toThrow("adapter");
  expect(f.calls()).toHaveLength(0);
});

test("confirm passes only the dispatch's own top-level transcript turns", async () => {
  const f = fixture("claude-good");
  const dir = transcript(f, workerPath, 1, [
    { type: "permission-mode" },
    injected(workerPath),
    turn(workerPath),
    turn(workerPath + "/sub"),
  ]);
  // Older session in the same checkout and a subagent with another model are not this dispatch.
  transcript(f, workerPath, 2, [injected(workerPath, "ctx_other"), turn(workerPath, "x")]);
  mkdirSync(join(dir, uuid(1), "subagents"), { recursive: true });
  writeFileSync(
    join(dir, uuid(1), "subagents/agent-a1.jsonl"),
    JSON.stringify(turn(workerPath, "claude-haiku-5-5")) + "\n",
    { mode: 0o600 },
  );
  const r = await f.c.confirm("ctx_fixture");
  expect(r).toMatchObject({ verdict: "PASS", transcripts: 1, assistant: 2, worktree: workerPath });
  expect(f.calls().map((a) => a.slice(1, 3).join(" "))).toEqual(["orchestration worker-show"]);
});
for (const [name, bad] of [
  ["model", turn(workerPath, "claude-fable-5-1")],
  ["effort", turn(workerPath, "claude-opus-5-5", "xhigh")],
] as const)
  test(`confirm fails on actual ${name} mismatch`, async () => {
    const f = fixture("claude-good");
    transcript(f, workerPath, 1, [injected(workerPath), turn(workerPath), bad]);
    await expect(f.c.confirm("ctx_fixture")).rejects.toThrow("mismatch in 1 of 2");
  });
test("confirm is PENDING without assistant turns and never waits", async () => {
  const f = fixture("claude-good");
  expect((await f.c.confirm("ctx_fixture")).verdict).toBe("PENDING");
  transcript(f, workerPath, 1, [injected(workerPath)]);
  // Tool output naming the dispatch is not the injected Task prompt.
  transcript(f, workerPath, 2, [
    { type: "user", cwd: workerPath, message: { content: [{ text: "ctx_fixture" }] } },
    turn(workerPath, "claude-fable-5-1"),
  ]);
  expect(await f.c.confirm("ctx_fixture")).toMatchObject({
    verdict: "PENDING",
    transcripts: 1,
    assistant: 0,
  });
});
for (const scenario of ["claude-wrongrun", "claude-effective", "good"])
  test(`confirm refuses ${scenario} dispatch/profile`, async () => {
    const f = fixture(scenario);
    transcript(f, workerPath, 1, [injected(workerPath), turn(workerPath)]);
    await expect(f.c.confirm("ctx_fixture")).rejects.toThrow();
  });
test("transcript mapping refuses unobserved path characters", () => {
  const f = fixture("claude-good");
  expect(() => f.c.sessions("/work/a.b_c")).toThrow("unverified transcript mapping");
});

function controllerTranscript(f: ReturnType<typeof fixture>, n: number, effort = "xhigh") {
  transcript(f, f.p.parent, n, [
    injected(f.p.parent, "ctx_controller"),
    turn(f.p.parent, "claude-opus-5-5", effort),
  ]);
}
test("claude controller inspect uses its own session transcript", async () => {
  const f = fixture("claudectl-good");
  controllerTranscript(f, 1);
  controllerTranscript(f, 2, "high");
  process.env.CLAUDE_CODE_SESSION_ID = uuid(1);
  try {
    const r = await f.c.inspect();
    expect(r.controllerEvidence).toBe("transcript");
    expect(r.controller.adapter).toBe("claude");
  } finally {
    delete process.env.CLAUDE_CODE_SESSION_ID;
  }
  // Without the controller's own session, two candidate transcripts are ambiguous.
  await expect(f.c.inspect()).rejects.toThrow("ambiguous");
});
test("claude controller from another caller needs exactly one transcript", async () => {
  const f = fixture("claudectl-good");
  controllerTranscript(f, 1);
  process.env.ORCA_TERMINAL_HANDLE = "term_reviewer";
  expect((await f.c.inspect()).controllerEvidence).toBe("transcript");
});
for (const [scenario, effort] of [
  ["claudectl-good", "high"],
  ["claudectl-good", null],
  // A screen line naming the model and effort is not evidence, even at the start of a line.
  ["claudectl-screen", null],
  ["claudectl-screen-prose", null],
  ["claudectl-screen", "high"],
  ["claudectl-ctl-codex", "xhigh"],
] as const)
  test(`claude controller refuses ${scenario} with transcript ${effort}`, async () => {
    const f = fixture(scenario);
    if (effort) controllerTranscript(f, 1, effort);
    await expect(f.c.inspect()).rejects.toThrow();
    expect(f.calls().some((a) => a.includes("worker-list"))).toBe(false);
  });

test("profile v2 states adapters explicitly; v1 and unknown values refuse", () => {
  const f = fixture(),
    path = join(f.root, "profile.json");
  const write = (value: object) => writeFileSync(path, JSON.stringify(value), { mode: 0o600 });
  const fable = { ...claudeOpus, model: "claude-fable-5-1", banner: "Fable 5.1" };
  const { codex, ...noCodex } = f.p;
  const claudeOnly = { ...noCodex, controller: claudeOpus, worker: claudeOpus };
  write({ ...claudeOnly, reviewer: fable, concurrency: null });
  expect(loadProfile(path).reviewer?.model).toBe("claude-fable-5-1");
  for (const bad of [
    { ...f.p, version: 1 },
    { ...f.p, worker: { ...f.p.worker, adapter: "grok" } },
    {
      ...f.p,
      worker: { model: "gpt-6.1-sol", effort: "high", tier: "default", banner: "GPT-6.1-Sol" },
    },
    { ...f.p, worker: { ...claudeOpus, tier: "fast" } },
    { ...f.p, worker: { ...claudeOpus, model: "opus" } },
    { ...noCodex },
    { ...f.p, concurrency: 0 },
  ]) {
    write(bad);
    expect(() => loadProfile(path)).toThrow();
  }
});

test("legacy worker with a claude profile starts through worker-start", async () => {
  const f = fixture("claude-legacy");
  prepareLegacy(f);
  expect((await legacy(["worker", "unit", "draft/fixture", sha], f.p, f.c)).dispatchId).toBe(
    "ctx_fixture",
  );
  const req = privateJson(join(f.root, "unit/request.json"));
  expect(req.worktree).toBe(join(f.root, "v6-unit"));
  const s = f.calls().find((a) => a.includes("worker-start"))!;
  expect(s[s.indexOf("--model") + 1]).toBe("claude-opus-5-5");
  expect(mutations(f)).toHaveLength(0);
  expect(
    JSON.parse(readFileSync(join(f.root, "unit/start.json"), "utf8")).result.preservedTerminals,
  ).toEqual(["term_startup"]);
});
test("legacy review uses the claude reviewer entry beside a codex worker", async () => {
  const f = fixture("claude-legacy");
  prepareLegacy(f);
  f.p.worker = {
    adapter: "codex",
    model: "gpt-6.1-sol",
    effort: "high",
    tier: "default",
    banner: "GPT-6.1-Sol",
  };
  f.p.reviewer = { ...claudeOpus, model: "claude-fable-5-1", banner: "Fable 5.1" };
  writeFileSync(f.p.reviewerCommon, "Reviewer-only fixture");
  const report = join(f.root, "unit/report.md");
  await legacy(["review", "unit", sha, report, "draft/fixture"], f.p, f.c);
  const req = privateJson(join(f.root, "unit/request.json"));
  expect(req.worktree).toBe(join(f.root, "rv-claude-unit"));
  expect(req.role).toBe("reviewer");
  const s = f.calls().find((a) => a.includes("worker-start"))!;
  expect([s[s.indexOf("--agent") + 1], s[s.indexOf("--model") + 1]]).toEqual([
    "claude",
    "claude-fable-5-1",
  ]);
  expect(readFileSync(join(f.root, "unit/dispatch-prompt.md"), "utf8")).toContain(
    "Reviewer-only fixture",
  );
  expect(mutations(f)).toHaveLength(0);
});
for (const kind of ["tier", "adapter", "alias", "effort"])
  test("legacy claude rejects " + kind + " before any mutation", async () => {
    const f = fixture("claude-legacy");
    prepareLegacy(f);
    if (kind === "tier") f.p.worker.tier = "fast";
    if (kind === "adapter") (f.p.worker as any).adapter = "cursor";
    if (kind === "alias") f.p.worker.model = "opus";
    if (kind === "effort") process.env.SOL_EFFORT = "max";
    try {
      await expect(legacy(["worker", "unit", "draft/fixture", sha], f.p, f.c)).rejects.toThrow();
    } finally {
      delete process.env.SOL_EFFORT;
    }
    expect(f.calls()).toHaveLength(0);
  });
test("inspect refuses a locator whose coordinator no longer holds the Run binding", async () => {
  const f = fixture("stalerun");
  await expect(f.c.inspect()).rejects.toThrow("takeover");
  expect(f.calls().some((a) => a.includes("worker-list") || a.includes("check"))).toBe(false);
});
