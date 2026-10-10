import { basename } from "node:path";

const NEW_CONTROL =
  "ordinary research plan persists|task widget retires|a late task-widget R1|owner releases opaque legacy reservations|a late legacy release";
const RECOVERY =
  "real browser offline start|a native committed pause|a planner A-B-A|an estimate A-B-A|a genuine new session retires|transient browser 429|one ordinary task restore";
const RESTART = "native same-database restart";

export function evidenceDirectory(
  runDir: string,
  current = process.env.FVOCI_W5_EVIDENCE_DIR,
): string {
  return current ? current : `${runDir}/playwright-output/w5-evidence`;
}

function explicitSelection(arg: string): boolean {
  return (
    arg === "--grep" ||
    arg.startsWith("--grep=") ||
    arg === "--grep-invert" ||
    arg.startsWith("--grep-invert=") ||
    arg === "-g" ||
    (arg.startsWith("-g") && arg.length > 2 && !arg.startsWith("--")) ||
    arg === "--shard" ||
    arg.startsWith("--shard=") ||
    arg === "--list" ||
    arg === "--"
  );
}

export function timerInvocations(
  args: string[],
  pending = process.env.FVOCI_E2E_PENDING,
): string[][] | null {
  let count = 0;
  let other = false;
  let explicit = false;
  for (const arg of args) {
    if (arg === "v050-task-timer.spec.ts" || arg === "e2e/v050-task-timer.spec.ts") count += 1;
    else if (arg.endsWith(".spec.ts")) other = true;
    else if (explicitSelection(arg)) explicit = true;
  }
  if (count !== 1 || other || explicit || pending === "1") return null;
  return [
    ["--grep-invert", `${NEW_CONTROL}|${RECOVERY}|${RESTART}`],
    ["--grep", RECOVERY],
    ["--grep", RESTART],
    ["--grep", NEW_CONTROL],
  ];
}

export function groupLabel(args: string[], pending = process.env.FVOCI_E2E_PENDING): string {
  const specs = args.filter((arg) => !arg.startsWith("-"));
  if (pending === "1" && specs.length < 1) return "collaboration-pending";
  const first = specs[0];
  if (!first) return "default-suite";
  const stem = (value: string) => basename(value).replace(/\.spec\.ts$/, "");
  const second = specs[1];
  return second ? `${stem(first)}+${stem(second)}` : stem(first);
}
