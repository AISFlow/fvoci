// Pure libtest receipt parsers. Raw child output may contain endpoint, query
// or token text: only literal closed codes are ever echoed, and no failure
// shape can become PASS.
import {
  DIAGNOSTIC_UNIT_NAME,
  INVENTORY_TEST_NAME,
  MIGRATION_TEST_NAME,
  RESET_TEST_NAME,
  TEST_NAME,
} from "./guard-policy.ts";
import {
  codePointLength as cpLength,
  PY_SPACE,
  pySplitlines as splitlines,
} from "../web-e2e/compat.ts";
import { DIGIT } from "./python-compat.ts";

/** Lines to print, then a refusal code (null = PASS). */
export interface Verdict {
  lines: string[];
  code: string | null;
}

const escape = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const whole = (pattern: string): RegExp => new RegExp("^(?:" + pattern + ")$", "u");
const count = (text: string, marker: string): number => text.split(marker).length - 1;
const OPAQUE_FAILED = /(?:^|\n)FAILED\r?(?:\n|$)/u;
const PASS_SUMMARY = new RegExp(
  "test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; " + DIGIT + "+ filtered out;",
  "u",
);

// Per-line match equivalent to a MULTILINE ^...\r?$ findall where only LF
// starts a line.
function lineMatches(output: string, pattern: string): string[][] {
  const line = whole(pattern);
  const found: string[][] = [];
  for (const segment of output.split("\n")) {
    const match = line.exec(segment);
    if (match) found.push(match.slice(1));
  }
  return found;
}

export function diagnosticListingSelected(status: number, listing: string): boolean {
  const matches = lineMatches(listing, "([^\\r\\n]+): (test|benchmark)\\r?");
  return (
    status === 0 &&
    matches.length === 1 &&
    matches[0]?.[0] === DIAGNOSTIC_UNIT_NAME &&
    matches[0][1] === "test"
  );
}

export function diagnosticUnitResult(status: number, output: string): Verdict {
  const cases = lineMatches(
    output,
    "test ([^" + PY_SPACE + "]+) \\.\\.\\. (ok|FAILED|ignored)\\r?",
  );
  const summaries = splitlines(output).filter((line) => line.startsWith("test result:"));
  const summary = whole(
    "test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; " +
      DIGIT +
      "+ filtered out;[^\\r\\n]*",
  );
  if (
    status !== 0 ||
    cases.length !== 1 ||
    cases[0]?.[0] !== DIAGNOSTIC_UNIT_NAME ||
    cases[0][1] !== "ok" ||
    summaries.length !== 1 ||
    !summary.test(summaries[0] as string)
  ) {
    return { lines: [], code: "TURSO_DIAGNOSTIC_UNIT_FAILED" };
  }
  // Raw listing/test/panic output is discarded, even on success.
  return { lines: ["TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN"], code: null };
}

const RESET_PRIMARY_CODES = new Set([
  "OK",
  "BEGIN_FAILED",
  "WRONG_PRODUCT_BACKEND",
  "WRONG_BACKEND",
  "CURRENT_LINEAGE_CHANGED",
  "FK_QUERY_FAILED",
  "FK_DECODE_FAILED",
  "FOREIGN_KEYS_NOT_ONE",
  "LITERAL_QUERY_FAILED",
  "LITERAL_DECODE_FAILED",
  "LITERAL_MISMATCH",
  "RESET_SCHEMA_REFUSED",
  "RESET_DDL_FAILED",
  "RESET_BLANK_IN_WRITER_FAILED",
  "COMMIT_UNCONFIRMED",
  "RESET_FRESH_BLANK_FAILED",
]);

const RESET_RECEIPT =
  "FVOCI_TURSO_RESET_RECEIPT primary=([A-Z_]+) " +
  "rollback=(NOT_STARTED|RETURNED_OK|UNCONFIRMED) " +
  "commit=(NOT_STARTED|RETURNED_OK|UNCONFIRMED) " +
  "blank=(NOT_RUN|CONFIRMED|FAILED) steps=(0|[1-9][0-9]{0,2}) " +
  "close=(OK|FAILED) drain=(LOCAL_OK|UNCONFIRMED) leases=(ZERO|FAILED)";
const RESET_FRAME = "\\n*running 1 test\\r?\\ntest " + escape(RESET_TEST_NAME) + " \\.\\.\\. ";
const RESET_SUCCESS = whole(
  RESET_FRAME +
    RESET_RECEIPT +
    "\\r?\\nok\\r?\\n\\r?\\n" +
    "test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; " +
    DIGIT +
    "+ filtered out; finished in " +
    DIGIT +
    "+(?:\\." +
    DIGIT +
    "+)?s\\r?\\n*",
);
const RESET_FAILURE = whole(
  RESET_FRAME +
    RESET_RECEIPT +
    "\\r?\\n\\r?\\nFVOCI_TURSO_RESET_RETURN\\r?\\n" +
    "([\\s\\S]{0,16384}?)FAILED\\r?\\n\\r?\\nfailures:\\r?\\n\\r?\\nfailures:\\r?\\n" +
    "[ \\t]+" +
    escape(RESET_TEST_NAME) +
    "\\r?\\n\\r?\\n" +
    "test result: FAILED\\. 0 passed; 1 failed; 0 ignored; 0 measured; " +
    DIGIT +
    "+ filtered out; finished in " +
    DIGIT +
    "+(?:\\." +
    DIGIT +
    "+)?s\\r?\\n*",
);
const HEALTHY_RESET = [
  "OK",
  "NOT_STARTED",
  "RETURNED_OK",
  "CONFIRMED",
  "126",
  "OK",
  "LOCAL_OK",
  "ZERO",
];
const RECEIPT_MARKERS = ["FVOCI_TURSO_", "test result:", "running ", "test ", "failures:"];

export function resetResult(status: number, output: string): Verdict {
  const success = RESET_SUCCESS.exec(output);
  if (
    status === 0 &&
    success &&
    success.slice(1).every((value, index) => value === HEALTHY_RESET[index])
  ) {
    return {
      lines: [
        "TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO",
        "TURSO_RESET_PASS tests=1 ignored=0",
      ],
      code: null,
    };
  }
  // A failed frame can disclose closed producer facts but never become PASS.
  const lines: string[] = [];
  const failed = status !== 0 && cpLength(output) <= 32768 ? RESET_FAILURE.exec(output) : null;
  if (failed) {
    const [primary, rollback, commit, blank, steps, close, drain, leases, opaque] = failed.slice(
      1,
    ) as [string, string, string, string, string, string, string, string, string];
    const settled =
      (commit === "NOT_STARTED" && blank === "NOT_RUN") ||
      (commit === "UNCONFIRMED" &&
        primary === "COMMIT_UNCONFIRMED" &&
        blank === "NOT_RUN" &&
        rollback === "NOT_STARTED") ||
      (commit === "RETURNED_OK" &&
        rollback === "NOT_STARTED" &&
        ((primary === "OK" && blank === "CONFIRMED") ||
          (primary === "RESET_FRESH_BLANK_FAILED" && blank === "FAILED")));
    const completed = Number(steps);
    const beforeEffect = ![
      "OK",
      "RESET_DDL_FAILED",
      "RESET_BLANK_IN_WRITER_FAILED",
      "COMMIT_UNCONFIRMED",
      "RESET_FRESH_BLANK_FAILED",
    ].includes(primary);
    const stage =
      (beforeEffect && completed === 0 && commit === "NOT_STARTED") ||
      (primary === "RESET_DDL_FAILED" && completed < 126 && commit === "NOT_STARTED") ||
      (primary === "RESET_BLANK_IN_WRITER_FAILED" &&
        completed === 126 &&
        commit === "NOT_STARTED") ||
      (["OK", "COMMIT_UNCONFIRMED", "RESET_FRESH_BLANK_FAILED"].includes(primary) &&
        completed === 126);
    const earlyPrimary = primary === "BEGIN_FAILED" || primary === "WRONG_PRODUCT_BACKEND";
    const rollbackShape =
      (earlyPrimary && rollback === "NOT_STARTED") ||
      (commit !== "NOT_STARTED" && rollback === "NOT_STARTED") ||
      (!earlyPrimary &&
        commit === "NOT_STARTED" &&
        (rollback === "RETURNED_OK" || rollback === "UNCONFIRMED"));
    if (
      RESET_PRIMARY_CODES.has(primary) &&
      completed <= 126 &&
      settled &&
      stage &&
      rollbackShape &&
      (close === "OK") === (drain === "LOCAL_OK") &&
      (primary !== "COMMIT_UNCONFIRMED" || commit === "UNCONFIRMED") &&
      (!(primary === "OK" || primary === "RESET_FRESH_BLANK_FAILED") || commit === "RETURNED_OK") &&
      !(primary === "OK" && close === "OK" && leases === "ZERO") &&
      count(output, "FVOCI_TURSO_RESET_RECEIPT") === 1 &&
      count(output, "FVOCI_TURSO_RESET_RETURN") === 1 &&
      !RECEIPT_MARKERS.some((marker) => opaque.includes(marker)) &&
      !OPAQUE_FAILED.test(opaque)
    ) {
      lines.push(
        "TURSO_RESET_FAILURE " +
          [
            "primary=" + primary,
            "rollback=" + rollback,
            "commit=" + commit,
            "blank=" + blank,
            "steps=" + steps,
            "close=" + close,
            "drain=" + drain,
            "leases=" + leases,
          ].join(" "),
      );
    }
  }
  return { lines, code: "TURSO_RESET_FAILED" };
}

export const INVENTORY_PRIMARY_CODES: ReadonlySet<string> = new Set([
  "BEGIN_FAILED",
  "WRONG_PRODUCT_BACKEND",
  "WRONG_BACKEND",
  "FK_QUERY_FAILED",
  "FK_DECODE_FAILED",
  "FOREIGN_KEYS_NOT_ONE",
  "LITERAL_QUERY_FAILED",
  "LITERAL_DECODE_FAILED",
  "LITERAL_MISMATCH",
  "CURRENT_LINEAGE_CHANGED",
  "INVENTORY_QUERY_FAILED",
  "INVENTORY_DECODE_FAILED",
  "INVENTORY_PREFIX_REFUSED",
  "INVENTORY_SCHEMA_REFUSED",
  "INVENTORY_SNAPSHOT_MISMATCH",
  "INVENTORY_HASH_INVALID",
]);

const INVENTORY_FAILURE = whole(
  "(?:\\r?\\n)*running 1 test\\r?\\n" +
    "test " +
    escape(INVENTORY_TEST_NAME) +
    " \\.\\.\\. " +
    "FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE " +
    "rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED) leases=(ZERO|FAILED)\\r?\\n\\r?\\n" +
    "FVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=([A-Z_]+) rollback=([A-Z_]+) " +
    "close=([A-Z_]+) leases=(ZERO|FAILED)\\r?\\n" +
    "FVOCI_TURSO_INVENTORY_RETURN\\r?\\n(?<harness>(?:[^\\n]*\\n)*?)" +
    "FAILED\\r?\\n(?:\\r?\\n)*failures:\\r?\\n(?:\\r?\\n)*failures:\\r?\\n" +
    "    " +
    escape(INVENTORY_TEST_NAME) +
    "\\r?\\n(?:\\r?\\n)*" +
    "test result: FAILED\\. 0 passed; 1 failed; 0 ignored; 0 measured; " +
    "[0-9]+ filtered out; finished in [0-9]+\\.[0-9]+s\\r?\\n(?:\\r?\\n)*",
);
const INVENTORY_ROLLBACK_RECEIPT: Record<string, string> = {
  OK: "OK",
  NOT_STARTED: "NOT_STARTED",
  ROLLBACK_UNCONFIRMED: "FAILED",
};

// Same LF/single-CRLF separator policy as the migration diagnostic. A static
// producer RETURN boundary separates facts from untrusted returned-Error bytes;
// no returned Error is parsed as a code or ACK.
function inventoryFailureDiagnostic(status: number, output: string): string[] {
  if (
    status === 0 ||
    cpLength(output) > 32768 ||
    [
      "FVOCI_TURSO_INVENTORY_RECEIPT",
      "FVOCI_TURSO_INVENTORY_DIAGNOSTIC",
      "FVOCI_TURSO_INVENTORY_RETURN",
    ].some((marker) => count(output, marker) !== 1)
  ) {
    return [];
  }
  const match = INVENTORY_FAILURE.exec(output);
  if (!match) return [];
  const [rollbackReceipt, closeReceipt, leaseReceipt, primary, rollback, close, leases, harness] =
    match.slice(1) as [string, string, string, string, string, string, string, string];
  // Opaque harness output may contain private errors; never reflect it or
  // adopt its framing. Extra producer markers/tests/results refuse.
  if (
    cpLength(harness) > 16384 ||
    RECEIPT_MARKERS.some((marker) => harness.includes(marker)) ||
    OPAQUE_FAILED.test(harness)
  ) {
    return [];
  }
  if (
    !(primary === "OK" || INVENTORY_PRIMARY_CODES.has(primary)) ||
    !["OK", "NOT_STARTED", "ROLLBACK_UNCONFIRMED"].includes(rollback) ||
    !["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"].includes(close) ||
    leases !== leaseReceipt ||
    rollbackReceipt !== INVENTORY_ROLLBACK_RECEIPT[rollback] ||
    (close === "OK") !== (closeReceipt === "OK") ||
    (primary === "BEGIN_FAILED" || primary === "WRONG_PRODUCT_BACKEND") !==
      (rollback === "NOT_STARTED") ||
    (primary === "OK" && rollback === "OK" && close === "OK" && leases === "ZERO")
  ) {
    return [];
  }
  return [
    "TURSO_INVENTORY_FAILURE classification=REFUSED prefix=NONE schema_sha256=NONE" +
      " rollback=" +
      rollbackReceipt +
      " close=" +
      closeReceipt +
      " leases=" +
      leaseReceipt,
    "TURSO_INVENTORY_DIAGNOSTIC primary=" +
      primary +
      " rollback=" +
      rollback +
      " close=" +
      close +
      " leases=" +
      leases,
  ];
}

// The maintained serial libtest --nocapture framing, literally: no stripping
// arbitrary lines, summary-only admission or raw failure reflection.
const INVENTORY_SUCCESS = whole(
  "(?:\\r?\\n)*running 1 test\\r?\\n" +
    "test " +
    escape(INVENTORY_TEST_NAME) +
    " \\.\\.\\. " +
    "FVOCI_TURSO_INVENTORY_RECEIPT classification=(BLANK|PREFIX|CURRENT) " +
    "prefix=(0|[1-9]|1[0-2]) schema_sha256=([0-9a-f]{64}) " +
    "rollback=OK close=OK leases=ZERO\\r?\\nok\\r?\\n" +
    "(?:\\r?\\n)*test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; " +
    "[0-9]+ filtered out; finished in [0-9]+\\.[0-9]+s\\r?\\n(?:\\r?\\n)*",
);

export function inventoryResult(status: number, output: string): Verdict {
  const match = INVENTORY_SUCCESS.exec(output);
  if (status !== 0 || !match) {
    return { lines: inventoryFailureDiagnostic(status, output), code: "TURSO_INVENTORY_FAILED" };
  }
  const [classification, prefix, schemaHash] = match.slice(1) as [string, string, string];
  const level = Number(prefix);
  if (!(
    (classification === "BLANK" && prefix === "0") ||
    (classification === "PREFIX" && level >= 1 && level <= 11) ||
    (classification === "CURRENT" && prefix === "12")
  )) {
    return { lines: [], code: "TURSO_INVENTORY_FAILED" };
  }
  return {
    lines: [
      "TURSO_INVENTORY_RECEIPT classification=" +
        classification +
        " prefix=" +
        prefix +
        " schema_sha256=" +
        schemaHash +
        " rollback=OK close=OK leases=ZERO",
      "TURSO_INVENTORY_PASS tests=1 ignored=0",
    ],
    code: null,
  };
}

// Literal consumer codes only: no arbitrary SDK/test output may be echoed.
export const MIGRATION_PRIMARY_CODES: ReadonlySet<string> = new Set([
  "BEGIN_FAILED",
  "CLOSE_FAILED",
  "COMMIT_UNCONFIRMED",
  "CURRENT_APPLY_FAILED",
  "CURRENT_GATE_FAILED",
  "CURRENT_GATE_MISMATCH",
  "CURRENT_LINEAGE_CHANGED",
  "DATA_DECODE_FAILED",
  "DATA_QUERY_FAILED",
  "DATA_WRITE_FAILED",
  "DATA_WRITE_MISMATCH",
  "DDL_FAILED",
  "DEFER_PRAGMA_REFUSED",
  "FENCE_BASELINE_NOT_EMPTY",
  "FENCE_ROW_UNBOUND",
  "FENCE_WRITE_FAILED",
  "FK_DECODE_FAILED",
  "FK_FAILURE_MISSING",
  "FK_QUERY_FAILED",
  "FK_ROLLBACK_PREFIX_CHANGED",
  "FOREIGN_KEYS_NOT_ONE",
  "GENERATION_WRITE_FAILED",
  "GENERATION_WRITE_MISMATCH",
  "INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED",
  "LEASES_NOT_ZERO",
  "LITERAL_DECODE_FAILED",
  "LITERAL_MISMATCH",
  "LITERAL_QUERY_FAILED",
  "NEGATIVE_REFUSAL_NOT_CONFIRMED",
  "NEGATIVE_ROLLBACK_CHANGED_CURRENT",
  "NEGATIVE_WRITE_FAILED",
  "NOT_FK_ONLY",
  "PARENT_PRESENT",
  "PREFIX_APPLY_FAILED",
  "PREFIX_RECEIPTS_CHANGED",
  "PREFIX_VALIDATION_FAILED",
  "PRESERVED_DATA_MISMATCH",
  "RECONNECT_FAILED",
  "RESTART_APPLY_FAILED",
  "RESTART_RECEIPTS_OR_SCHEMA_CHANGED",
  "ROLLBACK_UNCONFIRMED",
  "SCHEMA_VALIDATION_FAILED",
  "SEED_DECODE_FAILED",
  "SEED_MISMATCH",
  "SEED_QUERY_FAILED",
  "UNEXPECTED_TARGET_DATA",
  "WITNESS_DECODE_FAILED",
  "WITNESS_MISMATCH",
  "WITNESS_QUERY_FAILED",
  "WRONG_BACKEND",
  "WRONG_FK_FAILURE",
]);
export const MIGRATION_CLOSE_CODES: ReadonlySet<string> = new Set([
  "CLOSE_FAILED",
  "LEASES_NOT_ZERO",
]);
// Closed proof kinds of the shared FK rollback: the typed extended proof, or
// the exact primary-only Hrana code plus the same-writer row witness.
export const MIGRATION_FK_PROOF_KINDS: ReadonlySet<string> = new Set([
  "EXTENDED",
  "SAME_WRITER_PRIMARY_HRANA",
]);

const MIGRATION_RECEIPT =
  /FVOCI_TURSO_MIGRATION_RECEIPT primary=(OK|FAILED) prefix=(OK|NOT_CONFIRMED) fk_rollback=(OK|NOT_CONFIRMED) fk_proof=(EXTENDED|SAME_WRITER_PRIMARY_HRANA|NOT_CONFIRMED) current=(OK|NOT_CONFIRMED) restart=(OK|NOT_CONFIRMED) close=(OK|FAILED) leases=(ZERO|FAILED)(?:\r?\n|$)/gu;
const MIGRATION_DIAGNOSTIC =
  /^FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=([A-Z_]+) close=([A-Z_]+)$/u;
const MIGRATION_TEST = new RegExp("test " + escape(MIGRATION_TEST_NAME) + " \\.\\.\\. ", "u");

export function migrationResult(status: number, output: string): Verdict {
  const receipts = [...output.matchAll(MIGRATION_RECEIPT)].map((match) => match.slice(1));
  if (receipts.length !== 1) return { lines: [], code: "TURSO_MIGRATION_RECEIPT_MISSING" };
  const receipt = receipts[0] as string[];
  const lines = ["TURSO_MIGRATION_RECEIPT " + receipt.join(" ")];
  const fail = { lines, code: "TURSO_MIGRATION_FAILED" };
  // Inspect every occurrence, including malformed/private injected lines. A
  // diagnostic is never a success receipt and never authorizes a retry.
  const split = output.split("\n");
  const diagnostics = split
    .map((line, index) =>
      index < split.length - 1 && line.endsWith("\r") ? line.slice(0, -1) : line,
    )
    .filter((line) => line.includes("FVOCI_TURSO_MIGRATION_DIAGNOSTIC"));
  if (diagnostics.length > 0) {
    if (diagnostics.length !== 1 || receipt[0] !== "FAILED") return fail;
    const diagnostic = MIGRATION_DIAGNOSTIC.exec(diagnostics[0] as string);
    if (!diagnostic) return fail;
    const [primary, close] = diagnostic.slice(1) as [string, string];
    if (
      !(primary === "OK" || MIGRATION_PRIMARY_CODES.has(primary)) ||
      !(close === "OK" || MIGRATION_CLOSE_CODES.has(close)) ||
      (primary === "OK" && close === "OK") ||
      (close === "OK") !== (receipt[6] === "OK")
    ) {
      return fail;
    }
    lines.push("TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close);
  }
  // A confirmed run names exactly one closed proof kind; NOT_CONFIRMED never passes.
  if (
    status !== 0 ||
    receipt.slice(0, 3).join(" ") !== "OK OK OK" ||
    !MIGRATION_FK_PROOF_KINDS.has(receipt[3] as string) ||
    receipt.slice(4).join(" ") !== "OK OK OK ZERO" ||
    !PASS_SUMMARY.test(output) ||
    !MIGRATION_TEST.test(output)
  ) {
    return fail;
  }
  lines.push(
    "TURSO_MIGRATION_FK_PROOF kind=" + String(receipt[3]),
    "TURSO_MIGRATION_PASS tests=1 ignored=0",
  );
  return { lines, code: null };
}

const CONNECTION_RECEIPT =
  /FVOCI_TURSO_RECEIPT primary=([A-Z_]+) rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED|NOT_STARTED) leases=(ZERO|FAILED|NOT_OBSERVED)/gu;
const CONNECTION_PRIMARY = new Set([
  "OK",
  "CONNECT_FAILED",
  "BEGIN_FAILED",
  "WRONG_BACKEND",
  "FK_QUERY_FAILED",
  "FK_DECODE_FAILED",
  "FOREIGN_KEYS_NOT_ONE",
  "LITERAL_QUERY_FAILED",
  "LITERAL_DECODE_FAILED",
  "LITERAL_MISMATCH",
]);
const CONNECTION_TEST = new RegExp("test " + escape(TEST_NAME) + " \\.\\.\\. ", "u");

export function connectionResult(status: number, output: string): Verdict {
  const receipts = [...output.matchAll(CONNECTION_RECEIPT)].map((match) => match.slice(1));
  const receipt = receipts[0];
  if (receipts.length !== 1 || !receipt || !CONNECTION_PRIMARY.has(receipt[0] as string)) {
    return { lines: [], code: "TURSO_CONNECTION_RECEIPT_MISSING" };
  }
  const lines = ["TURSO_CONNECTION_RECEIPT " + receipt.join(" ")];
  if (
    status !== 0 ||
    receipt.join(" ") !== "OK OK OK ZERO" ||
    !PASS_SUMMARY.test(output) ||
    !CONNECTION_TEST.test(output)
  ) {
    return { lines, code: "TURSO_CONNECTION_FAILED" };
  }
  lines.push("TURSO_CONNECTION_PASS tests=1 ignored=0");
  return { lines, code: null };
}
