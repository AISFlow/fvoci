import { describe, expect, test } from "bun:test";
import {
  INVENTORY_TEST_NAME,
  MIGRATION_TEST_NAME,
  RESET_TEST_NAME,
  TEST_NAME,
} from "./guard-policy.ts";
import {
  inventoryFailure,
  inventoryHash,
  inventorySuccess,
  resetSuccess,
} from "./guard-fixtures.ts";
import {
  connectionResult,
  INVENTORY_PRIMARY_CODES,
  inventoryResult,
  MIGRATION_CLOSE_CODES,
  MIGRATION_FK_PROOF_KINDS,
  MIGRATION_PRIMARY_CODES,
  migrationResult,
  resetResult,
  type Verdict,
} from "./guard-receipts.ts";

// Separators that str.splitlines() treats as line ends but the receipts never do.
const SEPARATORS = [
  "\r",
  "\v",
  "\f",
  "\x1c",
  "\x1d",
  "\x1e",
  "\x85",
  String.fromCharCode(0x2028),
  String.fromCharCode(0x2029),
];
const printed = (verdict: Verdict): string => verdict.lines.map((line) => line + "\n").join("");
const swap = (text: string, from: string, to: string): string => text.split(from).join(to);

function refused(verdict: Verdict, code: string): string {
  expect(verdict.code).toBe(code);
  const output = printed(verdict);
  expect(output).not.toContain("FAKE_PRIVATE_TOKEN");
  expect(output).not.toContain("_PASS");
  return output;
}

describe("connection receipt", () => {
  const valid =
    "test " +
    TEST_NAME +
    " ... FVOCI_TURSO_RECEIPT primary=OK rollback=OK close=OK leases=ZERO\nok\n" +
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";

  test("zero-test, ignored, failed status or failed cleanup never passes", () => {
    expect(printed(connectionResult(0, valid))).toContain(
      "TURSO_CONNECTION_PASS tests=1 ignored=0",
    );
    for (const [raw, status] of [
      [valid.replace("1 passed", "0 passed"), 0],
      [valid.replace("0 ignored", "1 ignored"), 0],
      [valid, 1],
      [valid.replace("rollback=OK", "rollback=FAILED"), 0],
    ] as const) {
      refused(
        connectionResult(status, raw + "FAKE_PRIVATE_BODY_NEVER_PRINT"),
        "TURSO_CONNECTION_FAILED",
      );
      expect(printed(connectionResult(status, raw + "FAKE_PRIVATE_BODY"))).not.toContain(
        "FAKE_PRIVATE_BODY",
      );
    }
    for (const raw of ["", valid + valid, valid.replace("primary=OK", "primary=PRIVATE")]) {
      refused(connectionResult(0, raw), "TURSO_CONNECTION_RECEIPT_MISSING");
    }
  });
});

describe("migration receipt", () => {
  const valid =
    "test " +
    MIGRATION_TEST_NAME +
    " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\n" +
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const failure =
    "test " +
    MIGRATION_TEST_NAME +
    " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=FAILED prefix=NOT_CONFIRMED fk_rollback=NOT_CONFIRMED fk_proof=NOT_CONFIRMED current=NOT_CONFIRMED restart=NOT_CONFIRMED close=OK leases=ZERO\nFAILED\n" +
    "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out;\n";
  const failedOutput = (text: string, status = 1) =>
    refused(migrationResult(status, text), "TURSO_MIGRATION_FAILED");

  test("rejects missing, partial, wrong test and zero execution", () => {
    let output = printed(migrationResult(0, valid));
    expect(output).toContain("TURSO_MIGRATION_FK_PROOF kind=EXTENDED\n");
    expect(output).toContain("TURSO_MIGRATION_PASS tests=1 ignored=0");
    // The same-writer proof kind is the only other confirmed kind; it is echoed closed.
    output = printed(
      migrationResult(0, valid.replace("fk_proof=EXTENDED", "fk_proof=SAME_WRITER_PRIMARY_HRANA")),
    );
    expect(output).toContain("TURSO_MIGRATION_FK_PROOF kind=SAME_WRITER_PRIMARY_HRANA\n");
    expect(output).toContain("TURSO_MIGRATION_PASS tests=1 ignored=0");
    for (const [changed, code] of [
      [valid.replace("1 passed", "0 passed"), "TURSO_MIGRATION_FAILED"],
      // NOT_CONFIRMED never passes; missing, extra, unknown, generic, lower-case
      // or injected kinds are not a receipt at all.
      [valid.replace("fk_proof=EXTENDED", "fk_proof=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"],
      [valid.replace(" fk_proof=EXTENDED", ""), "TURSO_MIGRATION_RECEIPT_MISSING"],
      [
        valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED witness=PROVEN"),
        "TURSO_MIGRATION_RECEIPT_MISSING",
      ],
      [
        valid.replace("fk_proof=EXTENDED", "fk_proof=GENERIC_19"),
        "TURSO_MIGRATION_RECEIPT_MISSING",
      ],
      [
        valid.replace("fk_proof=EXTENDED", "fk_proof=SQLITE_CONSTRAINT"),
        "TURSO_MIGRATION_RECEIPT_MISSING",
      ],
      [valid.replace("fk_proof=EXTENDED", "fk_proof=extended"), "TURSO_MIGRATION_RECEIPT_MISSING"],
      [
        valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED_FAKE_PRIVATE_TOKEN"),
        "TURSO_MIGRATION_RECEIPT_MISSING",
      ],
      [valid.replace("fk_proof=EXTENDED", "fk_proof="), "TURSO_MIGRATION_RECEIPT_MISSING"],
      [valid.replace("0 ignored", "1 ignored"), "TURSO_MIGRATION_FAILED"],
      [swap(valid, MIGRATION_TEST_NAME, TEST_NAME), "TURSO_MIGRATION_FAILED"],
      [valid.replace("fk_rollback=OK", "fk_rollback=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"],
      [valid.replace("restart=OK", "restart=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"],
      [valid.replace("close=OK", "close=FAILED"), "TURSO_MIGRATION_FAILED"],
      [valid.replace("leases=ZERO", "leases=FAILED"), "TURSO_MIGRATION_FAILED"],
      [
        valid.replace("FVOCI_TURSO_MIGRATION_RECEIPT", "FAKE_RECEIPT"),
        "TURSO_MIGRATION_RECEIPT_MISSING",
      ],
      [valid + valid, "TURSO_MIGRATION_RECEIPT_MISSING"],
    ] as const) {
      refused(migrationResult(0, changed + "FAKE_PRIVATE_TOKEN_NEVER_PRINT"), code);
    }
    refused(migrationResult(1, valid), "TURSO_MIGRATION_FAILED");
  });

  test("exact static codes only and failures never become PASS", () => {
    // Contract extracted from the existing post-owner consumer failures.
    expect([...MIGRATION_PRIMARY_CODES].sort()).toEqual(
      "BEGIN_FAILED CLOSE_FAILED COMMIT_UNCONFIRMED CURRENT_APPLY_FAILED CURRENT_GATE_FAILED CURRENT_GATE_MISMATCH CURRENT_LINEAGE_CHANGED DATA_DECODE_FAILED DATA_QUERY_FAILED DATA_WRITE_FAILED DATA_WRITE_MISMATCH DDL_FAILED DEFER_PRAGMA_REFUSED FENCE_BASELINE_NOT_EMPTY FENCE_ROW_UNBOUND FENCE_WRITE_FAILED FK_DECODE_FAILED FK_FAILURE_MISSING FK_QUERY_FAILED FK_ROLLBACK_PREFIX_CHANGED FOREIGN_KEYS_NOT_ONE GENERATION_WRITE_FAILED GENERATION_WRITE_MISMATCH INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED LEASES_NOT_ZERO LITERAL_DECODE_FAILED LITERAL_MISMATCH LITERAL_QUERY_FAILED NEGATIVE_REFUSAL_NOT_CONFIRMED NEGATIVE_ROLLBACK_CHANGED_CURRENT NEGATIVE_WRITE_FAILED NOT_FK_ONLY PARENT_PRESENT PREFIX_APPLY_FAILED PREFIX_RECEIPTS_CHANGED PREFIX_VALIDATION_FAILED PRESERVED_DATA_MISMATCH RECONNECT_FAILED RESTART_APPLY_FAILED RESTART_RECEIPTS_OR_SCHEMA_CHANGED ROLLBACK_UNCONFIRMED SCHEMA_VALIDATION_FAILED SEED_DECODE_FAILED SEED_MISMATCH SEED_QUERY_FAILED UNEXPECTED_TARGET_DATA WITNESS_DECODE_FAILED WITNESS_MISMATCH WITNESS_QUERY_FAILED WRONG_BACKEND WRONG_FK_FAILURE".split(
        " ",
      ),
    );
    expect([...MIGRATION_CLOSE_CODES].sort()).toEqual(["CLOSE_FAILED", "LEASES_NOT_ZERO"]);
    expect([...MIGRATION_FK_PROOF_KINDS].sort()).toEqual(["EXTENDED", "SAME_WRITER_PRIMARY_HRANA"]);
    for (const kind of ["EXTENDED", "SAME_WRITER_PRIMARY_HRANA"]) {
      expect(
        failedOutput(failure.replace("fk_proof=NOT_CONFIRMED", "fk_proof=" + kind)),
      ).not.toContain("FK_PROOF");
    }
    for (const primary of MIGRATION_PRIMARY_CODES) {
      for (const close of ["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"]) {
        const receipt = close === "OK" ? failure : failure.replace("close=OK", "close=FAILED");
        const diagnostic =
          "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n";
        expect(failedOutput(receipt + diagnostic + "SDK FAKE_PRIVATE_TOKEN\n")).toContain(
          "TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n",
        );
      }
    }
    for (const close of ["CLOSE_FAILED", "LEASES_NOT_ZERO"]) {
      const output = failedOutput(
        failure.replace("close=OK", "close=FAILED") +
          "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=OK close=" +
          close +
          "\n",
      );
      expect(output).toContain("TURSO_MIGRATION_DIAGNOSTIC primary=OK close=" + close);
    }
  });

  test("unknown, malformed, duplicate or injected diagnostics do not echo", () => {
    const known = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n";
    for (const diagnostic of [
      known + known,
      known + "FVOCI_TURSO_MIGRATION_DIAGNOSTIC FAKE_PRIVATE_TOKEN\n",
      ...[
        "UNKNOWN_ERROR",
        "FAKE_PRIVATE_TOKEN",
        "SCHEMA_VALIDATION_FAILED_EXTRA",
        "schema_validation_failed",
        "CONNECT_FAILED",
        "MISSING_SECRET",
        "",
        "SCHEMA_VALIDATION_FAILED\nFAKE_PRIVATE_TOKEN",
        "SCHEMA_VALIDATION_FAILED\rFAKE_PRIVATE_TOKEN",
        "libsql://FAKE_PRIVATE_TOKEN.example.org",
        "OK",
      ].map((code) => known.replace("SCHEMA_VALIDATION_FAILED", code)),
      known.replace("close=OK", "close=BEGIN_FAILED"),
      known.replace("close=OK", "close=FAKE_PRIVATE_TOKEN"),
      known.replace("close=OK", "close=CLOSE_FAILED"),
      "FAKE_PRIVATE_TOKEN " + known,
      known.trimEnd() + " FAKE_PRIVATE_TOKEN\n",
      known.replace("primary=", "private="),
      known.replace(" close=", "\tclose="),
    ]) {
      expect(failedOutput(failure + diagnostic)).not.toContain("TURSO_MIGRATION_DIAGNOSTIC");
    }
    // A missing diagnostic still fails; it is never fabricated from raw Err.
    expect(failedOutput(failure + "SDK FAKE_PRIVATE_TOKEN\n")).not.toContain(
      "TURSO_MIGRATION_DIAGNOSTIC",
    );
  });

  test("a diagnostic cannot replace success execution or cleanup receipts", () => {
    const diagnostic =
      "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n";
    for (const [text, status] of [
      [valid + diagnostic, 0],
      [valid + diagnostic, 1],
      [failure + diagnostic, 0],
      [swap(failure, MIGRATION_TEST_NAME, TEST_NAME) + diagnostic, 0],
      [failure.replace("0 ignored", "1 ignored") + diagnostic, 0],
      [failure.replace("1 failed", "0 failed") + diagnostic, 0],
      [failure.replace("close=OK", "close=FAILED") + diagnostic, 0],
    ] as const) {
      failedOutput(text, status);
    }
    const output = printed(migrationResult(0, valid + "SDK FAKE_PRIVATE_TOKEN\n"));
    expect(output).toContain("TURSO_MIGRATION_PASS tests=1 ignored=0");
    expect(output).not.toContain("TURSO_MIGRATION_DIAGNOSTIC");
    expect(output).not.toContain("FAKE_PRIVATE_TOKEN");
  });

  test("only LF and a single CRLF delimit diagnostic lines", () => {
    const diagnostic = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK";
    for (const separator of SEPARATORS) {
      expect(failedOutput(failure + diagnostic + separator + "FAKE_PRIVATE_TOKEN\n")).not.toContain(
        "TURSO_MIGRATION_DIAGNOSTIC",
      );
    }
    for (const ending of ["\n", "\r\n"]) {
      expect(failedOutput(failure + diagnostic + ending)).toContain(
        "TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n",
      );
    }
    for (const ending of ["\r", "\r\r\n"]) {
      expect(failedOutput(failure + diagnostic + ending)).not.toContain(
        "TURSO_MIGRATION_DIAGNOSTIC",
      );
    }
  });
});

describe("inventory receipt", () => {
  const deniedResult = (text: string, status = 0) => {
    const verdict = inventoryResult(status, text);
    expect(verdict.code).toBe("TURSO_INVENTORY_FAILED");
    expect(verdict.lines).toEqual([]);
  };

  test("exact class, prefix, receipt and nocapture framing", () => {
    for (let prefix = 0; prefix < 13; prefix += 1) {
      const classification = prefix === 0 ? "BLANK" : prefix === 12 ? "CURRENT" : "PREFIX";
      for (const ending of ["\n", "\r\n"]) {
        expect(
          printed(inventoryResult(0, swap(inventorySuccess(classification, prefix), "\n", ending))),
        ).toBe(
          "TURSO_INVENTORY_RECEIPT classification=" +
            classification +
            " prefix=" +
            String(prefix) +
            " schema_sha256=" +
            inventoryHash +
            " rollback=OK close=OK leases=ZERO\nTURSO_INVENTORY_PASS tests=1 ignored=0\n",
        );
      }
    }
  });

  test("missing, duplicate, extra, wrong test count or partial summary refuses", () => {
    const valid = inventorySuccess();
    const receipt = (valid.split(" ... ")[1] as string).split("\n")[0] as string;
    for (const changed of [
      "",
      receipt + "\n",
      valid.slice(valid.indexOf("test result:")),
      valid.replace(receipt, ""),
      valid.replace(receipt, receipt + "\n" + receipt),
      valid + valid,
      swap(valid, INVENTORY_TEST_NAME, MIGRATION_TEST_NAME),
      valid.replace("running 1 test", "running 0 tests"),
      valid.replace("running 1 test", "running 2 tests"),
      valid.replace("1 passed", "0 passed"),
      valid.replace("1 passed", "2 passed"),
      valid.replace("0 failed", "1 failed"),
      valid.replace("0 ignored", "1 ignored"),
      valid.replace("0 measured", "1 measured"),
      valid.replace("100 filtered out", "100 filtered"),
      valid.replace(" finished in 0.00s", ""),
      valid.replace("\nok\n", "\nignored\n"),
      valid.replace("\nok\n", "\nFAILED\n"),
      valid.replace("running 1 test\n", ""),
      valid.replace("\nok\n", "\ntest other::case ... ok\nok\n"),
      valid + "test other::case ... ignored\n",
      valid.replace("test result: ok.", "test result: FAILED."),
      valid +
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
      "FAKE_PRIVATE_TOKEN " + valid,
      valid + "SDK FAKE_PRIVATE_TOKEN\n",
      valid.replace(" ... ", " ... FAKE_PRIVATE_TOKEN "),
      valid.replace("\nok\n", "\nSDK FAKE_PRIVATE_TOKEN\nok\n"),
      swap(valid, "\n", "\r"),
      swap(valid, "\n", String.fromCharCode(0x2028)),
    ]) {
      deniedResult(changed);
    }
  });

  test("class, prefix, hash and each cleanup field are not summary oracles", () => {
    const valid = inventorySuccess();
    for (const changed of [
      valid.replace("CURRENT", "UNKNOWN"),
      valid.replace("CURRENT", "current"),
      valid.replace("CURRENT", "BLANK"),
      valid.replace("CURRENT", "PREFIX"),
      ...["0", "11", "13", "012", "-1", "NONE"].map((prefix) =>
        valid.replace("prefix=12", "prefix=" + prefix),
      ),
      ...[
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        "libsql://FAKE_PRIVATE_TOKEN",
        "FAKE_PRIVATE_TOKEN\n" + inventoryHash,
      ].map((hash) => valid.replace(inventoryHash, hash)),
      valid.replace("rollback=OK", "rollback=FAILED"),
      valid.replace("rollback=OK", "rollback=NOT_STARTED"),
      valid.replace("close=OK", "close=FAILED"),
      valid.replace("close=OK", "close=NOT_STARTED"),
      valid.replace("leases=ZERO", "leases=FAILED"),
      valid.replace("leases=ZERO", "leases=NOT_OBSERVED"),
      valid.replace("FVOCI_TURSO_INVENTORY_RECEIPT", "FVOCI_TURSO_MIGRATION_RECEIPT"),
      valid.replace("leases=ZERO", "leases=ZERO FAKE_PRIVATE_TOKEN"),
      valid.replace(" schema_sha256=", "\tschema_sha256="),
      valid.replace(
        "CURRENT prefix=12 schema_sha256=" + inventoryHash,
        "REFUSED prefix=NONE schema_sha256=NONE",
      ),
      inventorySuccess("BLANK", 1),
      inventorySuccess("PREFIX", 0),
      inventorySuccess("PREFIX", 12),
    ]) {
      deniedResult(changed);
    }
    deniedResult(valid, 1);
    deniedResult(valid, -9);
    for (const separator of SEPARATORS) {
      deniedResult(
        valid.replace("leases=ZERO\n", "leases=ZERO" + separator + "FAKE_PRIVATE_TOKEN\n"),
      );
    }
  });
});

describe("inventory failure diagnostic", () => {
  const failedOutput = (text: string, status = 1): string => {
    const output = refused(inventoryResult(status, text), "TURSO_INVENTORY_FAILED");
    expect(output).not.toContain("opaque returned error");
    return output;
  };

  test("current codes and exact refused settled cleanup disclose only the failure", () => {
    expect([...INVENTORY_PRIMARY_CODES].sort()).toEqual(
      "BEGIN_FAILED CURRENT_LINEAGE_CHANGED FK_DECODE_FAILED FK_QUERY_FAILED FOREIGN_KEYS_NOT_ONE INVENTORY_DECODE_FAILED INVENTORY_HASH_INVALID INVENTORY_PREFIX_REFUSED INVENTORY_QUERY_FAILED INVENTORY_SCHEMA_REFUSED INVENTORY_SNAPSHOT_MISMATCH LITERAL_DECODE_FAILED LITERAL_MISMATCH LITERAL_QUERY_FAILED WRONG_BACKEND WRONG_PRODUCT_BACKEND".split(
        " ",
      ),
    );
    for (const code of [...INVENTORY_PRIMARY_CODES].sort()) {
      const rollbacks =
        code === "BEGIN_FAILED" || code === "WRONG_PRODUCT_BACKEND"
          ? ["NOT_STARTED"]
          : ["OK", "ROLLBACK_UNCONFIRMED"];
      for (const rollback of rollbacks) {
        for (const close of ["OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"]) {
          for (const leases of ["ZERO", "FAILED"]) {
            for (const ending of ["\n", "\r\n"]) {
              const output = failedOutput(
                swap(inventoryFailure(code, rollback, close, leases), "\n", ending),
              );
              expect(output).toContain(
                "TURSO_INVENTORY_DIAGNOSTIC primary=" +
                  code +
                  " rollback=" +
                  rollback +
                  " close=" +
                  close +
                  " leases=" +
                  leases +
                  "\n",
              );
              expect(output).toContain("classification=REFUSED prefix=NONE schema_sha256=NONE");
            }
          }
        }
      }
    }
  });

  test("primary OK records only a real finish or local lease failure, not all healthy", () => {
    for (const [rollback, close, leases] of [
      ["ROLLBACK_UNCONFIRMED", "OK", "ZERO"],
      ["ROLLBACK_UNCONFIRMED", "CLOSE_FAILED", "FAILED"],
      ["OK", "CLOSE_FAILED", "ZERO"],
      ["OK", "LEASES_NOT_ZERO", "FAILED"],
      ["OK", "OK", "FAILED"],
    ] as const) {
      expect(failedOutput(inventoryFailure("OK", rollback, close, leases))).toContain(
        "primary=OK rollback=" + rollback + " close=" + close + " leases=" + leases,
      );
    }
    for (const [primary, rollback] of [
      ["OK", "OK"],
      ["OK", "NOT_STARTED"],
      ["BEGIN_FAILED", "OK"],
      ["INVENTORY_QUERY_FAILED", "NOT_STARTED"],
    ] as const) {
      expect(failedOutput(inventoryFailure(primary, rollback))).toBe("");
    }
  });

  test("unknown, private, malformed and duplicate codes keep the cause unknown", () => {
    const valid = inventoryFailure();
    const diagnostic = (valid.split("\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC")[1] as string).split(
      "\n",
    )[0] as string;
    for (const code of [
      "UNKNOWN",
      "CONNECT_FAILED",
      "DDL_FAILED",
      "COMMIT_UNCONFIRMED",
      "SCHEMA_VALIDATION_FAILED",
      "",
      "inventory_schema_refused",
      "INVENTORY_SCHEMA_REFUSED_EXTRA",
      "FAKE_PRIVATE_TOKEN",
      "libsql://FAKE_PRIVATE_TOKEN",
    ]) {
      expect(
        failedOutput(valid.replace("primary=INVENTORY_SCHEMA_REFUSED", "primary=" + code)),
      ).toBe("");
    }
    for (const changed of [
      valid.replace(
        "primary=INVENTORY_SCHEMA_REFUSED",
        "primary=INVENTORY_SCHEMA_REFUSED\nFAKE_PRIVATE_TOKEN",
      ),
      valid.replace("rollback=OK close=OK", "rollback=UNKNOWN close=OK"),
      valid.replace(
        "close=OK leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN",
        "close=BEGIN_FAILED leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN",
      ),
      valid.replace(
        "FVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic,
        "FVOCI_TURSO_INVENTORY_DIAGNOSTIC" +
          diagnostic +
          "\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC" +
          diagnostic,
      ),
      valid.replace(
        "FVOCI_TURSO_INVENTORY_RETURN",
        "FVOCI_TURSO_INVENTORY_RETURN\nFVOCI_TURSO_INVENTORY_RETURN",
      ),
      valid.replace(
        "\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC",
        "\n\nFAKE_PRIVATE_TOKEN FVOCI_TURSO_INVENTORY_DIAGNOSTIC",
      ),
      valid.replace(
        "\nFVOCI_TURSO_INVENTORY_RETURN",
        " FAKE_PRIVATE_TOKEN\nFVOCI_TURSO_INVENTORY_RETURN",
      ),
      valid.replace("primary=INVENTORY_SCHEMA_REFUSED", "private=INVENTORY_SCHEMA_REFUSED"),
      valid.replace("\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic + "\n", "\n"),
      valid.replace("FVOCI_TURSO_INVENTORY_RETURN", "FAKE_RETURN"),
    ]) {
      expect(failedOutput(changed)).toBe("");
    }
  });

  test("a failure cannot forge pass counts, status, wrong case or cleanup tuple", () => {
    const valid = inventoryFailure();
    for (const [changed, status] of [
      [valid, 0],
      [valid.replace("0 passed", "1 passed"), 1],
      [valid.replace("1 failed", "0 failed"), 1],
      [valid.replace("0 ignored", "1 ignored"), 1],
      [valid.replace("0 measured", "1 measured"), 1],
      [valid.replace("running 1 test", "running 2 tests"), 1],
      [swap(valid, INVENTORY_TEST_NAME, MIGRATION_TEST_NAME), 1],
      [valid.replace("classification=REFUSED", "classification=CURRENT"), 1],
      [valid.replace("prefix=NONE", "prefix=12"), 1],
      [valid.replace("schema_sha256=NONE", "schema_sha256=" + "a".repeat(64)), 1],
      [
        valid.replace(
          "rollback=OK close=OK leases=ZERO\n\n",
          "rollback=FAILED close=OK leases=ZERO\n\n",
        ),
        1,
      ],
      [
        valid.replace(
          "rollback=OK close=OK leases=ZERO\n\n",
          "rollback=OK close=FAILED leases=ZERO\n\n",
        ),
        1,
      ],
      [
        valid.replace(
          "close=OK leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN",
          "close=OK leases=FAILED\nFVOCI_TURSO_INVENTORY_RETURN",
        ),
        1,
      ],
      [valid + valid, 1],
      [valid.slice(valid.indexOf("test result:")), 1],
      [valid.replace("test result: FAILED.", "test result: ok."), 1],
      [valid.replace("\nFAILED\n", "\nok\n"), 1],
      [inventorySuccess() + valid, 1],
      [valid + "test other::case ... ok\n", 1],
    ] as const) {
      expect(failedOutput(changed, status)).toBe("");
    }
  });

  test("the static RETURN boundary never interprets or exposes error text", () => {
    for (const harness of [
      'Error: "FAKE_PRIVATE_TOKEN"\n',
      "libsql://FAKE_PRIVATE_TOKEN\n",
      'Error: "CLOSE_FAILED"\n',
      "unqualified future harness framing FAKE_PRIVATE_TOKEN\n",
      "",
    ]) {
      const output = failedOutput(
        inventoryFailure(undefined, undefined, undefined, undefined, harness),
      );
      expect(output).toContain("primary=INVENTORY_SCHEMA_REFUSED rollback=OK close=OK leases=ZERO");
      expect(output).not.toContain("primary=CLOSE_FAILED");
    }
    for (const forged of [
      "test other::case ... ok\n",
      "test result: ok. 1 passed\n",
      "running 2 tests\n",
      "FAILED\n",
      "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=DDL_FAILED close=OK\n",
      "x".repeat(16385) + "\n",
    ]) {
      expect(
        failedOutput(inventoryFailure(undefined, undefined, undefined, undefined, forged)),
      ).toBe("");
    }
    expect(failedOutput(inventoryFailure() + "x".repeat(32769))).toBe("");
    // Caps count code points: 8200 astral characters are 16400 UTF-16 units.
    const astral = String.fromCodePoint(0x1f600).repeat(8200) + "\n";
    expect(
      failedOutput(inventoryFailure(undefined, undefined, undefined, undefined, astral)),
    ).toContain("TURSO_INVENTORY_DIAGNOSTIC");
  });

  test("only LF and a single CRLF delimit diagnostic producer lines", () => {
    const valid = inventoryFailure();
    for (const separator of SEPARATORS) {
      expect(
        failedOutput(
          valid.replace(
            "leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN",
            "leases=ZERO" + separator + "FAKE_PRIVATE_TOKEN\nFVOCI_TURSO_INVENTORY_RETURN",
          ),
        ),
      ).toBe("");
    }
    for (const ending of ["\r", "\r\r\n"]) {
      expect(
        failedOutput(
          valid.replace("\nFVOCI_TURSO_INVENTORY_RETURN", ending + "FVOCI_TURSO_INVENTORY_RETURN"),
        ),
      ).toBe("");
    }
  });
});

describe("reset receipt", () => {
  test("success requires the original complete execution and settlement", () => {
    const valid = resetSuccess();
    expect(printed(resetResult(0, valid))).toBe(
      "TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO\n" +
        "TURSO_RESET_PASS tests=1 ignored=0\n",
    );
    for (const [text, status] of [
      [valid, 1],
      [valid.replace("1 passed", "0 passed"), 0],
      [swap(valid, RESET_TEST_NAME, MIGRATION_TEST_NAME), 0],
      [valid.replace("commit=RETURNED_OK", "commit=UNCONFIRMED"), 0],
      [valid.replace("blank=CONFIRMED", "blank=NOT_RUN"), 0],
      [valid.replace("steps=126", "steps=125"), 0],
      [valid.replace("close=OK", "close=FAILED"), 0],
      [valid.replace("drain=LOCAL_OK", "drain=UNCONFIRMED"), 0],
      [valid.replace("leases=ZERO", "leases=FAILED"), 0],
      [valid + valid, 0],
      [valid + "FAKE_PRIVATE_TOKEN\n", 0],
      [valid.slice(valid.indexOf("test result:")), 0],
    ] as const) {
      const verdict = resetResult(status, text);
      expect(verdict).toEqual({ lines: [], code: "TURSO_RESET_FAILED" });
    }
  });

  test("failed primary, unknown commit or cleanup never becomes PASS", () => {
    const template = (fields: string) =>
      "\nrunning 1 test\ntest " +
      RESET_TEST_NAME +
      " ... FVOCI_TURSO_RESET_RECEIPT " +
      fields +
      "\n\nFVOCI_TURSO_RESET_RETURN\nError: FAKE_PRIVATE_TOKEN\nFAILED\n\nfailures:\n\nfailures:\n    " +
      RESET_TEST_NAME +
      "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n";
    for (const fields of [
      "primary=RESET_SCHEMA_REFUSED rollback=RETURNED_OK commit=NOT_STARTED blank=NOT_RUN steps=0 close=OK drain=LOCAL_OK leases=ZERO",
      "primary=RESET_DDL_FAILED rollback=UNCONFIRMED commit=NOT_STARTED blank=NOT_RUN steps=23 close=FAILED drain=UNCONFIRMED leases=ZERO",
      "primary=COMMIT_UNCONFIRMED rollback=NOT_STARTED commit=UNCONFIRMED blank=NOT_RUN steps=126 close=FAILED drain=UNCONFIRMED leases=ZERO",
      "primary=RESET_FRESH_BLANK_FAILED rollback=NOT_STARTED commit=RETURNED_OK blank=FAILED steps=126 close=OK drain=LOCAL_OK leases=ZERO",
      "primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=FAILED drain=UNCONFIRMED leases=ZERO",
    ]) {
      const text = template(fields);
      expect(resetResult(1, text)).toEqual({
        lines: ["TURSO_RESET_FAILURE " + fields],
        code: "TURSO_RESET_FAILED",
      });
      for (const mutated of [
        text.replace("primary=", "primary=PRIVATE_"),
        text.replace("0 passed; 1 failed", "1 passed; 0 failed"),
        text.replace(/steps=\d+/, "steps=127"),
        text.replace("FVOCI_TURSO_RESET_RETURN", "FAKE_PRIVATE_TOKEN"),
        text + text,
      ]) {
        expect(resetResult(1, mutated)).toEqual({ lines: [], code: "TURSO_RESET_FAILED" });
      }
    }
  });
});
