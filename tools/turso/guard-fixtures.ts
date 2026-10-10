// Recorded libtest framings shared by the guard tests (test support only).
import { INVENTORY_TEST_NAME, RESET_TEST_NAME } from "./guard-policy.ts";

export const inventoryHash = "a".repeat(64);
export function inventorySuccess(classification = "CURRENT", prefix = 12): string {
  return (
    "\nrunning 1 test\ntest " +
    INVENTORY_TEST_NAME +
    " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=" +
    classification +
    " prefix=" +
    String(prefix) +
    " schema_sha256=" +
    inventoryHash +
    " rollback=OK close=OK leases=ZERO\nok\n\n" +
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n"
  );
}

export function inventoryFailure(
  primary = "INVENTORY_SCHEMA_REFUSED",
  rollback = "OK",
  close = "OK",
  leases = "ZERO",
  harness: string | null = null,
): string {
  const receiptRollback = (
    { OK: "OK", NOT_STARTED: "NOT_STARTED", ROLLBACK_UNCONFIRMED: "FAILED" } as Record<
      string,
      string
    >
  )[rollback];
  // Illustrative libtest returned Error, never the cause oracle.
  const returned =
    primary !== "OK"
      ? primary
      : rollback !== "OK"
        ? rollback
        : close !== "OK"
          ? close
          : leases === "FAILED"
            ? "LEASES_NOT_ZERO"
            : "INVENTORY_DISCLOSURE_REFUSED";
  return (
    "\nrunning 1 test\ntest " +
    INVENTORY_TEST_NAME +
    " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE" +
    " rollback=" +
    String(receiptRollback) +
    " close=" +
    (close === "OK" ? "OK" : "FAILED") +
    " leases=" +
    leases +
    "\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=" +
    primary +
    " rollback=" +
    rollback +
    " close=" +
    close +
    " leases=" +
    leases +
    "\nFVOCI_TURSO_INVENTORY_RETURN\n" +
    (harness ?? 'Error: "' + returned + '"\n') +
    "FAILED\n\nfailures:\n\nfailures:\n    " +
    INVENTORY_TEST_NAME +
    "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n"
  );
}
export function resetSuccess(): string {
  return (
    "\nrunning 1 test\ntest " +
    RESET_TEST_NAME +
    " ... FVOCI_TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK " +
    "blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO\nok\n\n" +
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n"
  );
}
