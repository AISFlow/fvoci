import { afterAll, describe, expect, test } from "bun:test";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { SLOT_NAMES, unfilledSlots, verifyWorkflows } from "../verify-workflows.ts";
import {
  contextFromTexts,
  get,
  loadContext,
  parseWorkflow,
  pyRepr,
  pyReprList,
  triggersOf,
} from "./load.ts";
import { verifyWorkflowRegistry } from "./registry.ts";
import { TURSO_COMMANDS, verifyTursoWorkflow, verifyTursoWorkflowText } from "./turso.ts";

const root = resolve(import.meta.dir, "../../..");
const cli = join(root, "tools/ci/verify-workflows.ts");
const scratch = mkdtempSync(join(tmpdir(), "fvoci-verify-"));
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function workflowText(name: string): string {
  return readFileSync(join(root, ".github/workflows", name), "utf8");
}

function tree(files: Record<string, string>): string {
  const dir = mkdtempSync(join(scratch, "tree-"));
  mkdirSync(join(dir, ".github/workflows"), { recursive: true });
  for (const [name, text] of Object.entries(files))
    writeFileSync(join(dir, ".github/workflows", name), text);
  return dir;
}

function runCli(args: string[]): { code: number; stdout: string; stderr: string } {
  const result = Bun.spawnSync(["bun", cli, ...args], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

function runBash(
  script: string,
  env: Record<string, string>,
): { code: number; stdout: string; stderr: string } {
  const result = Bun.spawnSync(["bash", "-euo", "pipefail"], {
    stdin: new TextEncoder().encode(script),
    env: { PATH: "/usr/bin:/bin", ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

describe("WorkflowRegistryTest", () => {
  test("workflows match the planner registry", () => {
    expect(verifyWorkflows(loadContext(root))).toEqual([]);
  });

  test("turso UI cache paths are exported from the runner, not job env", () => {
    const ctx = loadContext(root);
    const data = ctx.workflows["turso-test.yml"] ?? {};
    expect(verifyTursoWorkflow(data)).toEqual([]);
    const ui = get(get(data, "jobs"), "turso-ui");
    for (const name of ["BUN_INSTALL_CACHE_DIR", "PLAYWRIGHT_BROWSERS_PATH"]) {
      expect(Object.hasOwn(get(ui, "env") as object, name)).toBe(false);
    }
    const steps = get(ui, "steps") as unknown[];
    const preparation =
      (get(steps[1], "run") as string).split("rustup toolchain install", 1)[0] ?? "";
    const runnerTemp = join(scratch, "runner temp with spaces");
    const envFile = join(scratch, "environment file with spaces");
    const result = runBash(preparation, { RUNNER_TEMP: runnerTemp, GITHUB_ENV: envFile });
    expect(result.code, result.stderr).toBe(0);
    expect(readFileSync(envFile, "utf8").split("\n").filter(Boolean)).toEqual([
      `CARGO_TARGET_DIR=${runnerTemp}/turso-ui-target`,
      `BUN_INSTALL_CACHE_DIR=${runnerTemp}/turso-bun-cache`,
      `PLAYWRIGHT_BROWSERS_PATH=${runnerTemp}/turso-browsers`,
    ]);
  });
});

describe("turso-test.yml literal text (turso-test-fixtures WorkflowTargetTests)", () => {
  const text = workflowText("turso-test.yml");

  test("the real workflow passes every literal check", () => {
    expect(verifyTursoWorkflowText(text)).toEqual([]);
  });

  test("the connection target is published before preparation, for later steps", () => {
    const prefix =
      "set -euo pipefail\n" +
      `printf 'CARGO_TARGET_DIR=%s/turso-target\\n' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n`;
    expect(text).toContain(
      prefix
        .split("\n")
        .map((line) => (line ? "          " + line : line))
        .join("\n"),
    );
    const directory = mkdtempSync(join(scratch, "turso-env-"));
    const envFile = join(directory, "github-env");
    const result = runBash(prefix, { RUNNER_TEMP: directory, GITHUB_ENV: envFile });
    expect([result.code, result.stdout, result.stderr]).toEqual([0, "", ""]);
    expect(readFileSync(envFile, "utf8")).toBe(`CARGO_TARGET_DIR=${directory}/turso-target\n`);
  });

  test("a secret reference in a comment before the consuming step is rejected", () => {
    const changed = text.replace("jobs:\n", "# uses secrets.X\njobs:\n");
    expect(verifyTursoWorkflowText(changed)).toContain(
      "turso-test.yml: text: no secret before the secret step",
    );
  });

  test("the diagnostic unit after the secret step is rejected by text order", () => {
    const unit = TURSO_COMMANDS.diagnosticUnit;
    const changed = text.replace(unit, "true").replace("# No upload", `# ${unit}\n# No upload`);
    expect(verifyTursoWorkflowText(changed)).toContain(
      "turso-test.yml: text: freeze, then the diagnostic unit, then the secret step",
    );
  });

  test("the UI reviewed ref cannot admit the connection job", () => {
    const changed = text.replace(
      "  turso-ui:\n",
      "    # refs/heads/fvoci/v060-product-integration-20261005\n  turso-ui:\n",
    );
    const errors = verifyTursoWorkflowText(changed);
    expect(errors).toContain(
      "turso-test.yml: text: UI reviewed ref never admits the connection job",
    );
  });
});

describe("YAML loader semantics the workflows rely on", () => {
  test("`on` is a string key, quoted or not", () => {
    for (const source of ["on:\n  push:\n", '"on":\n  push:\n']) {
      const parsed = parseWorkflow("x.yml", source);
      expect(parsed.data && triggersOf(parsed.data)).toEqual({ push: null });
    }
  });

  test("YAML 1.1-only scalars stay as the runner reads them", () => {
    const parsed = parseWorkflow("x.yml", "a: no\nb: off\nc: 017\nd: false\ne: ''\nf: '1'\ng: 4\n");
    expect(parsed.data).toEqual({ a: "no", b: "off", c: 17, d: false, e: "", f: "1", g: 4 });
  });

  test("anchors, aliases and literal blocks resolve; duplicate keys keep the last", () => {
    const parsed = parseWorkflow(
      "x.yml",
      "a: &x {k: [1]}\nb: *x\nc: |\n  one\n  two\nd: 1\nd: 2\n",
    );
    expect(parsed.data).toEqual({ a: { k: [1] }, b: { k: [1] }, c: "one\ntwo\n", d: 2 });
  });

  test("empty, scalar and list documents are not workflow mappings", () => {
    for (const source of ["", "text\n", "- a\n"]) {
      expect(parseWorkflow("x.yml", source)).toEqual({
        error: "x.yml: workflow YAML must be a mapping",
      });
    }
  });

  test("a syntax error is a parse failure for that file", () => {
    expect(parseWorkflow("x.yml", "a: [1, 2\n").error).toStartWith("x.yml: YAML parse failed: ");
  });

  test("a __proto__ key never reaches the prototype", () => {
    const parsed = parseWorkflow("x.yml", "__proto__: {jobs: {}}\n");
    expect(get(parsed.data, "jobs")).toBeUndefined();
  });

  test("Python repr quoting in messages", () => {
    expect(pyRepr("needs.ci-plan.outputs.select_x == 'true'")).toBe(
      "\"needs.ci-plan.outputs.select_x == 'true'\"",
    );
    expect(pyRepr("a\\b\n")).toBe("'a\\\\b\\n'");
    expect(pyRepr(`both ' and "`)).toBe(`'both \\' and "'`);
    expect(pyReprList([])).toBe("[]");
    expect(pyReprList(["contents", "issues"])).toBe("['contents', 'issues']");
    expect(pyRepr("a\x85b\u2028c\u00a0d\u200be")).toBe("'a\\x85b\\u2028c\\xa0d\\u200be'");
  });
});

describe("registry boundaries beyond the mutation table", () => {
  const texts = Object.fromEntries(
    [
      "rust.yml",
      "web.yml",
      "install.yml",
      "documents.yml",
      "collab-engine.yml",
      "release.yml",
      "ci-base-image.yml",
      "turso-test.yml",
    ].map((name) => [name, workflowText(name)]),
  );
  const errors = (changes: Record<string, string | null>): string[] => {
    const merged: Record<string, string> = {};
    for (const [name, text] of Object.entries({ ...texts, ...changes }))
      if (text !== null) merged[name] = text;
    return verifyWorkflowRegistry(contextFromTexts(root, merged));
  };

  test("missing, unparsable and non-mapping registered files are reported per workflow", () => {
    expect(errors({ "install.yml": null })).toContain("install: missing workflow file install.yml");
    expect(errors({ "web.yml": "" })).toContain("web: web.yml: workflow YAML must be a mapping");
    expect(
      errors({ "release.yml": "a: [\n" }).some((e) =>
        e.startsWith("release.yml: release.yml: YAML parse failed: "),
      ),
    ).toBe(true);
    expect(errors({ "ci-base-image.yml": "- x\n" })).toContain(
      "ci-base-image.yml: ci-base-image.yml: workflow YAML must be a mapping",
    );
  });

  test("a numeric scalar where false is pinned is not false", () => {
    const web = String(texts["web.yml"]).replace(
      "          fetch-depth: 0",
      "          fetch-depth: false",
    );
    expect(errors({ "web.yml": web })).toContain(
      "web: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override",
    );
  });

  test("a cache step whose with is not a mapping is reported, not skipped", () => {
    const collab = String(texts["collab-engine.yml"]).replace(
      "# v4\n        with:\n          path: ~/.cargo/registry\n          key: ",
      "# v4\n        with:\n        x-key: ",
    );
    expect(errors({ "collab-engine.yml": collab })).toContain(
      "collab-engine.yml: native-collab-engine cache with must be a mapping",
    );
  });

  test("a non-string cache key list cannot hide an unqualified entry", () => {
    const collab = String(texts["collab-engine.yml"]).replace(
      "restore-keys: v1-collab-engine-ubuntu-26.04-",
      "restore-keys:\n            - x\n            - v1-collab-engine-ubuntu-26.04-",
    );
    expect(errors({ "collab-engine.yml": collab })).toContain(
      "collab-engine.yml: native-collab-engine cache restore-keys must bind Ubuntu 26.04, architecture and toolchain",
    );
  });
});

describe("verify-workflows CLI", () => {
  test("passes on the repository with --partial and prints nothing", () => {
    const result = runCli(["--partial"]);
    expect(result).toEqual({ code: 0, stdout: "", stderr: "" });
  });

  test("fails closed while a check slot is not wired", () => {
    const result = runCli([]);
    const missing = unfilledSlots();
    expect(result.code).toBe(missing.length > 0 ? 1 : 0);
    expect(result.stderr.split("\n").filter(Boolean)).toEqual(
      missing.map((name) => `verify-workflows: check slot ${name} is not wired`),
    );
    expect(SLOT_NAMES).toHaveLength(9);
  });

  test("reports one error per line on stderr and exits 1", () => {
    const dir = tree({ "extra.yml": "on: push\njobs: {}\n" });
    const result = runCli([`--repo-root=${dir}`, "--partial"]);
    expect(result.code).toBe(1);
    expect(result.stdout).toBe("");
    const lines = result.stderr.split("\n").filter(Boolean);
    expect(lines).toContain("unknown workflow file extra.yml");
    expect(lines).toContain("rust: missing workflow file rust.yml");
  });

  test("a missing workflow directory is one error", () => {
    const dir = mkdtempSync(join(scratch, "empty-"));
    const result = runCli(["--repo-root", dir, "--partial"]);
    expect([result.code, result.stderr]).toEqual([1, "missing .github/workflows directory\n"]);
  });

  test("loads through the CLI path: CRLF passes, a dangling link is not a file", () => {
    const directory = join(root, ".github/workflows");
    const files = Object.fromEntries(
      readdirSync(directory).map((name) => [
        name,
        readFileSync(join(directory, name), "utf8").replaceAll("\n", "\r\n"),
      ]),
    );
    const dir = tree(files);
    symlinkSync(join(dir, "missing-target.yml"), join(dir, ".github/workflows/old.yml"));
    expect(runCli(["--repo-root", dir, "--partial"])).toEqual({ code: 0, stdout: "", stderr: "" });
  });

  test("a leading BOM stays in the workflow text and YAML skips it", () => {
    const text = workflowText("documents.yml");
    const dir = tree({ "documents.yml": "\ufeff" + text });
    const ctx = loadContext(dir);
    expect(ctx.loadErrors).toEqual({});
    expect(ctx.texts["documents.yml"]).toBe("\ufeff" + text);
    expect(ctx.workflows["documents.yml"]).toEqual(
      contextFromTexts(root, { "documents.yml": text }).workflows["documents.yml"] ?? {},
    );
  });

  test("a job whose steps are not a list is reported", () => {
    const text = workflowText("documents.yml").replace(
      "\njobs:\n",
      "\njobs:\n  documents-extra:\n    runs-on: ubuntu-26.04\n    steps:\n",
    );
    const errors = verifyWorkflowRegistry(contextFromTexts(root, { "documents.yml": text }));
    expect(errors).toContain("documents.yml: documents-extra steps must be a list");
  });

  test("usage errors exit 2", () => {
    expect(runCli(["--bogus"]).code).toBe(2);
    expect(runCli(["--repo-root"]).code).toBe(2);
    expect(runCli(["--repo-root", "--partial"]).code).toBe(2);
  });
});
