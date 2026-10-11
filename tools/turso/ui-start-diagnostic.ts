// Bounded, redacted classification of a failed normal-main start. No source
// or error text is ever copied into the public record, even a recognized line.
import { closeSync, openSync, readSync } from "node:fs";

export const SERVER_BUDGET = 10;
export const START_DIAGNOSTIC_INPUT_CAP = 16 * 1024;
// Canonical, redacted texts from db/migrate.rs GATE_TEXTS.
export const START_GATE_TEXTS: Record<string, readonly string[]> = {
  none: ["driver error withheld"],
  GATE_AHEAD_INCOMPLETE_UNPREPARED: ["SQLite schema is ahead, incomplete or unprepared"],
  GATE_GAP_FOREIGN_DIGEST: ["SQLite schema has a gap, foreign lineage or changed digest"],
  GATE_CATALOG_DIFFERS: [
    "SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused",
  ],
  GATE_RETIRED_LINEAGE: [
    "SQLite schema carries the retired development lineage; install into an empty database",
  ],
  GATE_STEP_GAP: ["SQLite migration gap"],
  GATE_REFERENCE_PIN: ["SQLite schema reference engine pin mismatch"],
  GATE_FAMILY_HANDLE: [
    "SQLite schema gate requires an actual SQLite-family handle",
    "SQLite migrations require an actual SQLite-family handle",
  ],
  GATE_BACKEND_KIND: [
    "remote migrator requires the libsql-remote backend",
    "remote startup requires the libsql-remote backend",
  ],
  GATE_ENDPOINT_SHAPE: ["remote libSQL requires a TLS primary endpoint and token"],
  GATE_CANCELLED: ["SQLite migration cancelled at a checkpoint"],
  GATE_VALIDATION_FAILED: ["schema validation failed"],
};
export const START_REMOTE_CATEGORIES: Record<string, string> = {
  REMOTE_CONNECT_REFUSED: "remote-connect-refused",
  REMOTE_SCHEMA_GATE_REFUSED: "remote-schema-gate-refused",
  REMOTE_MIGRATION_STEP_FAILED: "remote-migration-step-failed",
  REMOTE_MIGRATION_COMMIT_UNKNOWN: "remote-migration-commit-unknown",
  REMOTE_MIGRATION_CANCELLED: "remote-migration-cancelled",
  REMOTE_DRAIN_FAILED: "remote-drain-failed",
};

const escape = (value: string) => value.replace(/[.*+?^${}()|[\]\\/-]/g, "\\$&");
const own = (table: Record<string, unknown>, key: string) => Object.hasOwn(table, key);

export function startupRemoteCause(line: string): [string, string] | null {
  // Rust main's boxed String error uses Debug (quoted); preparation uses
  // Display. Match the whole producer line and its complete closed tuple.
  const wrappers = [
    [
      "fvoci: preparation failed; the server does not start: remote preparation refused ",
      "prepare-remote",
    ],
    ['Error: "remote normal startup refused ', "server-remote"],
  ] as const;
  for (const [prefix, phase] of wrappers) {
    if (!line.startsWith(prefix)) continue;
    let tail = line.slice(prefix.length);
    if (phase === "server-remote") {
      if (!tail.endsWith('"')) return null;
      tail = tail.slice(0, -1);
    }
    const match = /^\(([A-Z_]+), gate ([A-Z_]+|none), settlement ([a-z-]+)\): ([^\n]+)$/.exec(tail);
    if (match === null) return null;
    const [, code, gate, settlement, display] = match as unknown as [
      string,
      string,
      string,
      string,
      string,
    ];
    if (!own(START_REMOTE_CATEGORIES, code) || !own(START_GATE_TEXTS, gate)) return null;
    const number = /^remote migration (?:step (-?[0-9]+)|cancelled after ([0-9]+)) /.exec(display);
    if (number !== null) {
      const value = (number[1] ?? number[2]) as string;
      const parsed = BigInt(value);
      if (
        String(parsed) !== value ||
        !(number[1] !== undefined
          ? -(2n ** 31n) <= parsed && parsed < 2n ** 31n
          : 0n <= parsed && parsed < 2n ** 64n)
      )
        return null;
    }
    if (
      code === "REMOTE_CONNECT_REFUSED" &&
      !["none", "GATE_BACKEND_KIND", "GATE_ENDPOINT_SHAPE"].includes(gate)
    )
      return null;
    if (
      code === "REMOTE_MIGRATION_CANCELLED" &&
      settlement === "cancel-checkpoint-settled" &&
      gate !== "none"
    )
      return null;
    const drain = "; remote stream drain failed at close";
    const tailDrain = settlement === "drain-failed" ? drain : "";
    const texts = START_GATE_TEXTS[gate] as readonly string[];
    let patterns: string[] = [];
    if (code === "REMOTE_CONNECT_REFUSED" && settlement === "no-write-opened") {
      patterns = [
        escape("remote libSQL primary connect refused (TLS endpoint and token required)"),
      ];
    } else if (code === "REMOTE_SCHEMA_GATE_REFUSED") {
      const stages: Record<string, string[]> = {
        "no-write-opened": [
          "remote schema gate refused before any write",
          "remote startup gate refused",
        ],
        "writes-may-have-committed": ["remote schema gate refused after migration steps committed"],
        "drain-failed": [
          "remote schema gate refused before any write",
          "remote startup gate refused",
          "remote schema gate refused after migration steps committed",
        ],
      };
      patterns = (own(stages, settlement) ? (stages[settlement] as string[]) : []).flatMap(
        (stage) => texts.map((item) => escape(stage + ": " + item + tailDrain)),
      );
    } else if (code === "REMOTE_MIGRATION_STEP_FAILED") {
      const details: Record<string, string[]> = {
        "rollback-confirmation-withheld": ["rollback confirmation withheld"],
        "cleanup-unconfirmed": ["cleanup unconfirmed, admission quarantined"],
        "drain-failed": [
          "rollback confirmation withheld",
          "cleanup unconfirmed, admission quarantined",
        ],
      };
      patterns = (own(details, settlement) ? (details[settlement] as string[]) : []).flatMap(
        (detail) =>
          texts.map(
            (item) =>
              "remote migration step -?[0-9]{1,10} failed; " +
              escape(detail + ": " + item + tailDrain),
          ),
      );
    } else if (
      code === "REMOTE_MIGRATION_COMMIT_UNKNOWN" &&
      (settlement === "commit-unknown" || settlement === "drain-failed")
    ) {
      patterns = [
        "remote migration step -?[0-9]{1,10}" +
          escape(
            " commit outcome is unknown; settlement receipt retained; rerun resumes from the ledger" +
              tailDrain,
          ),
      ];
    } else if (
      code === "REMOTE_MIGRATION_CANCELLED" &&
      (settlement === "cancel-checkpoint-settled" || settlement === "drain-failed")
    ) {
      patterns = [
        "remote migration cancelled after [0-9]{1,20}" + escape(" settled step(s)" + tailDrain),
      ];
    } else if (code === "REMOTE_DRAIN_FAILED" && settlement === "drain-failed") {
      patterns = [escape("remote stream drain failed at close")];
    }
    if (patterns.some((pattern) => new RegExp("^(?:" + pattern + ")$").test(display)))
      return [phase, START_REMOTE_CATEGORIES[code] as string];
  }
  return null;
}

export interface StartDiagnostic {
  originalFailure: "UI_SERVER_START_FAILED";
  diagnosticStatus: "unqualified" | "qualified";
  processState: "unclassified" | "exited" | "deadline";
  exitCode: number | null;
  elapsedMs: number | null;
  phase: string | null;
  category: string | null;
}

/** `poll` is the child's exit observation: an exit code, null while running. */
export function serverStartDiagnostic(
  poll: (() => unknown) | null,
  logpath: string | null,
  started: number | null,
  deadline: number | null,
  now: () => number = () => performance.now() / 1000,
): StartDiagnostic {
  const result: StartDiagnostic = {
    originalFailure: "UI_SERVER_START_FAILED",
    diagnosticStatus: "unqualified",
    processState: "unclassified",
    exitCode: null,
    elapsedMs: null,
    phase: null,
    category: null,
  };
  try {
    const code = poll !== null ? poll() : null;
    const at = now();
    if (typeof code === "number" && Number.isInteger(code) && code >= -255 && code <= 255) {
      result.processState = "exited";
      result.exitCode = code;
    } else if (poll !== null && code === null && deadline !== null && at >= deadline)
      result.processState = "deadline";
    if (started !== null && Number.isFinite(at - started))
      result.elapsedMs = Math.max(
        0,
        Math.min(SERVER_BUDGET * 1000, Math.trunc((at - started) * 1000)),
      );
    if (logpath === null) throw new TypeError("no log");
    const buffer = new Uint8Array(START_DIAGNOSTIC_INPUT_CAP + 1);
    const fd = openSync(logpath, "r");
    let size = 0;
    try {
      for (;;) {
        const n = readSync(fd, buffer, size, buffer.length - size, null);
        if (!n) break;
        size += n;
        if (size === buffer.length) break;
      }
    } finally {
      closeSync(fd);
    }
    const raw = buffer.subarray(0, size);
    if (size > START_DIAGNOSTIC_INPUT_CAP || !size || raw[size - 1] !== 0x0a) return result;
    if (raw.some((byte) => byte > 0x7f)) return result;
    const lines = Buffer.from(raw).toString("latin1").split("\n").slice(0, -1);
    const causes: [string, string][] = [];
    const missing = new Set([
      "fvoci: ENCRYPTION_KEYS is not set (see the env example)",
      "fvoci: ENCRYPTION_ACTIVE_KEY_ID is not set (see the env example)",
    ]);
    for (let line of lines) {
      if (line.endsWith("\r")) line = line.slice(0, -1);
      if (missing.has(line)) causes.push(["prepare-config", "missing-encryption-keyring"]);
      else if (
        line !== "fvoci: not starting; fix .env and run docker compose up -d again" &&
        line !== "fvoci: prepared; starting the server"
      ) {
        const cause = startupRemoteCause(line);
        if (cause === null) return result;
        causes.push(cause);
      }
    }
    if (causes.length && new Set(causes.map((cause) => cause.join("\0"))).size === 1) {
      result.diagnosticStatus = "qualified";
      [result.phase, result.category] = causes[0] as [string, string];
    }
  } catch {
    // The record stays unqualified; the caller keeps the original failure.
  }
  return result;
}
