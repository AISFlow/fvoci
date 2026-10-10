// Runner entry for footer.test.ts only. It keeps the real CLI parser and the
// real permissions and owner-return bodies, and replaces the Git allocation
// identity, the browser copy and the lane drivers. No product, database,
// container or browser starts. It does not run the production main(): the
// output physical/owner/0700 and argument-combination checks of main() are
// covered by runner.test.ts against the real CLI, not by the footer runs.
import { strict as assert } from "node:assert";
import { mkdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import type { parseCLI } from "../../scripts/run-selected-backend-e2e.ts";
import { runtimeAccess } from "./admission.ts";
import { gid, groups, inventory, read, sha, uid, write } from "./io.ts";
import { ownershipReturn, runtimePermissions, selectedRuns } from "./runtime.ts";
import type { Inputs } from "./types.ts";

export const fixtureOwner = "owned-permission-fixture";
export interface FixtureInputs extends Inputs {
  fixture_bun: string;
  fixture_chrome: string;
  fixture_exit: number;
  fixture_incomplete: boolean;
  fixture_fault: string;
}
const closureFaults = [
  "closed",
  "wrong-source",
  "missing-process",
  "live",
  "wrong-flow",
  "dropped-off",
  "duplicate-root",
  "foreign-owner",
  "missing-port",
  "invalid-port",
  "missing-pid",
  "unsafe-canary",
];
const identity = () => fixtureOwner;

// The selected launcher body: real 1000 access check, then the lane receipts
// a driver would leave for owner-return, mutated by the requested fault.
function fixtureRun(output: string): number {
  assert.ok(uid() === 1000 && gid() === 1000);
  const facts = statSync(output);
  assert.ok(facts.uid === 1000 && (facts.mode & 0o777) === 0o700);
  const before = read(join(output, "before.json")) as FixtureInputs,
    fault = before.fixture_fault,
    chromium = before.fixture_chrome;
  runtimeAccess(Object.keys(before.external), {
    bun: { path: before.fixture_bun, sha256: sha(before.fixture_bun) },
    chromium: { path: chromium, sha256: sha(chromium) },
    chromium_directory_files: inventory(dirname(chromium)),
  });
  write(join(output, "fixture-marker.json"), { uid: uid(), gid: gid(), groups: groups() });
  if (before.fixture_incomplete) {
    mkdirSync(join(output, "runtime"));
    write(join(output, "install-allocation.json"), {});
  }
  if (closureFaults.includes(fault)) {
    const runtime = join(output, "runtime");
    mkdirSync(runtime);
    const runs: {
      lane: string;
      flow: string;
      actualSource: string;
      runRoot: string;
      exit: unknown;
    }[] = [];
    for (const [lane, flow] of selectedRuns) {
      const runRoot = join(runtime, "root-current-" + lane + "-" + flow + "-fixture");
      mkdirSync(runRoot);
      write(join(output, lane + "-" + flow + "-allocation.json"), {});
      const receipt: Record<string, unknown> = {
        source: before.head,
        tree: before.tree,
        root_owner: fixtureOwner,
        selected_flow: flow,
        final_exit_code: 0,
        owned_container_absent: true,
        owned_loopback_port_closed: true,
        recorded_process_identities_retired: true,
        cleanup_errors: [],
      };
      if (lane === "install") {
        receipt.actual_owned_process_receipts = 15;
        const retained = join(runRoot, "retained-run");
        mkdirSync(retained);
        for (let index = 0; index < 15; index++)
          write(join(retained, String(index) + "-process.json"), {
            status: fault === "missing-process" && index === 0 ? null : 0,
          });
      }
      if (lane === "postgres")
        write(join(runRoot, "parent-receipt.json"), {
          source: before.head,
          tree: before.tree,
          root_owner: fixtureOwner,
          selected_flow: flow,
          all_owned_fixtures_closed: true,
        });
      if (fault === "wrong-source") receipt.source = "foreign-source";
      if (fault === "live") receipt.recorded_process_identities_retired = false;
      if (fault === "wrong-flow" && flow === "off") receipt.selected_flow = "on";
      if (fault === "foreign-owner") receipt.root_owner = "foreign";
      if (lane === "sqlite") {
        receipt.final_exit_code = before.fixture_exit;
        if (fault === "missing-port" || fault === "unsafe-canary")
          Reflect.deleteProperty(receipt, "owned_loopback_port_closed");
        if (fault === "invalid-port") receipt.owned_loopback_port_closed = "PRIVATE_CANARY_URL";
        if (fault === "missing-pid")
          Reflect.deleteProperty(receipt, "recorded_process_identities_retired");
        if (fault === "unsafe-canary") {
          receipt.original_driver_failure = {
            message: "PRIVATE_CANARY_URL secret=PRIVATE_CANARY_SECRET",
          };
          receipt.session = "PRIVATE_CANARY_SESSION";
          receipt.headers = { Authorization: "PRIVATE_CANARY_SECRET" };
        }
      }
      write(join(runRoot, "receipt.json"), receipt);
      runs.push({
        lane,
        flow,
        actualSource: before.head,
        runRoot,
        exit: receipt.final_exit_code,
      });
    }
    if (fault === "dropped-off") runs.pop();
    const last = runs.at(-1),
      third = runs[2];
    if (fault === "duplicate-root" && last && third) last.runRoot = third.runRoot;
    write(join(output, "selected-ci-receipt.json"), {
      owner: fixtureOwner,
      source: before.head,
      tree: before.tree,
      runs,
    });
  }
  return before.fixture_exit;
}

export function footerMain(parse: typeof parseCLI, argv: string[]): number {
  const args = parse(argv);
  assert.ok(args);
  switch (args.mode) {
    case "permissions":
      assert.ok(args.sqliteParent !== undefined && args.dockerGid !== undefined);
      runtimePermissions(args.output, args.sqliteParent, args.dockerGid, {
        identity,
        browser: (output) => (read(join(output, "before.json")) as FixtureInputs).fixture_chrome,
      });
      return 0;
    case "run":
      return fixtureRun(args.output);
    case "owner-return":
      ownershipReturn(args.output, [1000, 1000], args.lane, identity);
      return 0;
    default:
      throw new Error("mode outside the footer fixture");
  }
}
export function footerEntry(parse: typeof parseCLI): void {
  try {
    process.exitCode = footerMain(parse, process.argv.slice(2));
  } catch {
    process.stderr.write("selected backend admission failed; private inputs withheld\n");
    process.exitCode = 1;
  }
}
