// Config-list admission cohort for config-list.test.ts only. Run as the fixed
// 1000:1000 actor: it builds a consumed collaboration packet, a fixture Git
// checkout and a private browser copy in its own directory, then calls the
// real configListInputs once per case. Nothing is launched.
import { spawnSync } from "bun";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { browserInventory, configListInputs, expectedFiles } from "./admission.ts";
import type { Consumed } from "./admission.ts";
import type { Bundle } from "./types.ts";
import { call, groups, read, sha, write } from "./io.ts";

const base = mkdtempSync(join(tmpdir(), "fvoci-config-list-"));
const results: Record<string, string> = {};
try {
  const checkout = join(base, "source"),
    output = join(base, "output"),
    config = join(checkout, "config.ts");
  mkdirSync(join(checkout, "apps/web/dist"), { recursive: true });
  mkdirSync(output, { mode: 0o700 });
  writeFileSync(config, "fixed selected config");
  writeFileSync(join(checkout, "apps/web/dist/asset.js"), "fixed dist");
  call(["git", "init", "-q"], checkout);
  writeFileSync(join(checkout, ".git/info/exclude"), "apps/\nnode_modules/\n");
  call(["git", "add", "config.ts"], checkout);
  call(
    ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@invalid", "commit", "-qm", "f"],
    checkout,
  );
  const head = call(["git", "rev-parse", "HEAD"], checkout),
    tree = call(["git", "rev-parse", "HEAD^{tree}"], checkout);
  const bun = process.execPath;
  const external: Record<string, string> = { [bun]: sha(bun) };
  for (const name of ["@playwright/test", "playwright", "playwright-core"]) {
    const path = join(checkout, "node_modules", name, "package.json");
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify({ version: "1.63.0", bin: { playwright: "cli.js" } }));
    external[path] = sha(path);
  }
  const cli = join(checkout, "node_modules/playwright/cli.js"),
    pkg = join(checkout, "node_modules/playwright/package.json");
  writeFileSync(cli, "fixture official CLI");
  external[cli] = sha(cli);
  const before = {
    head,
    tree,
    status: "",
    tracked: { "config.ts": sha(config) },
    external,
    untracked: {},
  };
  const binaries: Record<string, { sha256: string }> = {};
  for (const name of ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"]) {
    const path = join(base, name);
    writeFileSync(path, "fixture coherent artifact", { mode: 0o555 });
    binaries[path] = { sha256: sha(path) };
  }
  const core = join(base, "libfvoci.rlib");
  writeFileSync(core, "fixture emitted core", { mode: 0o444 });
  const bundle = {
    source: head,
    tree,
    binaries,
    compiler_artifacts: [
      {
        target: { name: "fvoci_server" },
        profile: { test: false },
        features: ["api-schema", "db-tests"],
        filenames: [core],
      },
    ],
  };
  const receipts: Record<string, unknown> = {
    "before.json": before,
    "after.json": before,
    "bundle.json": bundle,
    "build-environment.json": { bun: "1.4.2" },
    "build-env-inputs.json": {},
    "compile-receipt.json": {},
    "web-receipt.json": {
      source: head,
      tree,
      dist_files: { "asset.js": sha(join(checkout, "apps/web/dist/asset.js")) },
    },
    "abi-receipt.json": { currentSource: head, host_runtime_files: {} },
  };
  for (const [name, value] of Object.entries(receipts)) write(join(output, name), value);
  for (const stage of ["main", "lib", "install", "engine"])
    for (const suffix of ["-stage.json", "-compiler.jsonl"])
      write(join(output, stage + suffix), {});
  const received: Consumed["received"] = {};
  for (const path of expectedFiles(bundle as unknown as Bundle, output, undefined, checkout)) {
    const facts = statSync(path);
    received[path] = { sha256: sha(path), inode: facts.ino, mode: facts.mode & 0o777 };
  }
  const env = {
    CI: "true",
    GITHUB_ACTIONS: "true",
    FVOCI_WEB_BUILD_PHASE: "consume",
    GITHUB_JOB: "collaboration-flow",
    GITHUB_SHA: head,
    GITHUB_REPOSITORY: "owned/repo",
    GITHUB_RUN_ID: "123",
    GITHUB_RUN_ATTEMPT: "1",
    PLAYWRIGHT_BROWSERS_PATH: join(output, "browser"),
  };
  const consumedPath = join(output, "handoff-consumed.json");
  write(consumedPath, {
    source: head,
    tree,
    repository: env.GITHUB_REPOSITORY,
    run: env.GITHUB_RUN_ID,
    attempt: env.GITHUB_RUN_ATTEMPT,
    full_current_physical_inputs_equal: true,
    fresh_dist_equal: true,
    received,
  });
  const accessPath = join(output, "runtime-access-stage.json");
  const access = {
    source: head,
    tree,
    owner: "github:owned/repo:123:1:collaboration-flow",
    runtime_uid: 1000,
    runtime_gid: 1000,
    groups: groups(),
    preflight_exit: 0,
    preflight: { missing: 0 },
  };
  write(accessPath, access);
  const browser = join(output, "browser"),
    component = join(browser, "chromium-123"),
    chrome = join(component, "chrome");
  mkdirSync(component, { recursive: true, mode: 0o700 });
  chmodSync(browser, 0o700);
  writeFileSync(chrome, "fixture private executable", { mode: 0o700 });
  write(join(output, "runtime-browser-stage.json"), {
    source: head,
    cache: browser,
    chromium: chrome,
    files: { "chromium-123": browserInventory(component, [1000, 1000]) },
    metadata: { "chromium-123": browserInventory(component, [1000, 1000], true) },
  });
  Object.assign(process.env, env);

  // Rewrite in place: received records keep the same inode.
  const rewrite = (path: string, text: string) => {
    writeFileSync(path, text);
  };
  function rebindExternal(change: (external: Record<string, string>) => void): void {
    const record = read(join(output, "before.json")) as typeof before;
    change(record.external);
    const consumed = read(consumedPath) as Consumed;
    for (const name of ["before.json", "after.json"]) {
      const path = join(output, name);
      rewrite(path, JSON.stringify(record, null, 2) + "\n");
      const entry = consumed.received[path];
      if (entry) entry.sha256 = sha(path);
    }
    rewrite(consumedPath, JSON.stringify(consumed, null, 2) + "\n");
  }
  function attempt(name: string, change?: () => void, restore?: () => void): void {
    change?.();
    try {
      const value = configListInputs(output, checkout);
      results[name] =
        value.before.head === head && value.modules["playwright/cli.js"] === sha(cli)
          ? "admitted"
          : "admitted-wrong";
    } catch (error) {
      results[name] = "refused:" + (error instanceof Error ? error.constructor.name : "thrown");
    } finally {
      restore?.();
    }
  }
  attempt("baseline");
  const original = readFileSync(config, "utf8");
  attempt(
    "tracked source changed",
    () => {
      rewrite(config, "changed config");
    },
    () => {
      rewrite(config, original);
    },
  );
  attempt(
    "access receipt groups differ",
    () => {
      rewrite(accessPath, JSON.stringify({ ...access, groups: [0] }));
    },
    () => {
      rewrite(accessPath, JSON.stringify(access));
    },
  );
  const environmentPath = join(output, "build-environment.json");
  attempt(
    "received file mode changed",
    () => {
      chmodSync(environmentPath, 0o644);
    },
    () => {
      chmodSync(environmentPath, 0o600);
    },
  );
  attempt(
    "private browser mode changed",
    () => {
      chmodSync(chrome, 0o755);
    },
    () => {
      chmodSync(chrome, 0o700);
    },
  );
  attempt(
    "prepare phase",
    () => {
      process.env.FVOCI_WEB_BUILD_PHASE = "prepare";
    },
    () => {
      process.env.FVOCI_WEB_BUILD_PHASE = "consume";
    },
  );
  attempt(
    "orca-local execution",
    () => {
      process.env.FVOCI_SELECTED_EXECUTION_MODE = "orca-local";
    },
    () => {
      Reflect.deleteProperty(process.env, "FVOCI_SELECTED_EXECUTION_MODE");
    },
  );
  attempt(
    "runtime directory exists",
    () => {
      mkdirSync(join(output, "runtime"));
    },
    () => {
      rmSync(join(output, "runtime"), { recursive: true });
    },
  );
  attempt(
    "CLI missing from recorded inputs",
    () => {
      rebindExternal((record) => {
        Reflect.deleteProperty(record, cli);
      });
    },
    () => {
      rebindExternal((record) => {
        record[cli] = sha(cli);
      });
    },
  );
  attempt(
    "CLI is a symlink",
    () => {
      renameSync(cli, join(base, "foreign-cli"));
      symlinkSync(join(base, "foreign-cli"), cli);
    },
    () => {
      unlinkSync(cli);
      renameSync(join(base, "foreign-cli"), cli);
    },
  );
  const pkgText = readFileSync(pkg, "utf8");
  attempt(
    "playwright bin is not the official CLI",
    () => {
      rewrite(pkg, JSON.stringify({ version: "1.63.0", bin: { playwright: "other.js" } }));
      rebindExternal((record) => {
        record[pkg] = sha(pkg);
      });
    },
    () => {
      rewrite(pkg, pkgText);
      rebindExternal((record) => {
        record[pkg] = sha(pkg);
      });
    },
  );
  const cliText = readFileSync(cli, "utf8");
  attempt(
    "CLI bytes changed",
    () => {
      rewrite(cli, "changed unqualified CLI");
    },
    () => {
      rewrite(cli, cliText);
    },
  );
  const consumedText = readFileSync(consumedPath, "utf8");
  attempt(
    "consumed receipt missing",
    () => {
      unlinkSync(consumedPath);
    },
    () => {
      writeFileSync(consumedPath, consumedText, { mode: 0o600 });
    },
  );
  attempt("baseline after restores");
  results.launched = String(spawnSync(["test", "-e", join(output, "config-list")]).exitCode === 0);
} catch (error) {
  results.fixture = "failed:" + (error instanceof Error ? error.message.slice(0, 200) : "thrown");
} finally {
  rmSync(base, { recursive: true, force: true });
}
process.stdout.write(JSON.stringify(results));
