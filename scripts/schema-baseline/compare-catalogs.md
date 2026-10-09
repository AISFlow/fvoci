# compare-catalogs.ts

Compares two PostgreSQL catalog dumps. The ledger table's own columns, constraints, indexes, and rows are the only allowed difference, and only when the old side is the retired `(version, applied_at)` shape and the new side is contiguous `fvoci-postgres-060` receipts. Ledger ACLs, RLS, triggers, policies, column ACLs, and the app role's identity and grants stay strict. Exit 0 only when no semantic difference remains. Callers read that status and the markdown report on stdout and `--report`.

Where a comparison finishes, the report bytes match `compare-catalogs.py`. The rows below are the remaining cases where this program is stricter or stops earlier.

| Intent | Original | New | Why |
| --- | --- | --- | --- |
| Identical `NaN` values inside a list are not a catalog change | Container equality treats `[NaN]` as equal to `[NaN]`, so `extensions` does not become a DIFF | `NaN` is unequal to itself, so the same list is `DIFF extensions old=[nan] new=[nan]` and the run fails | A quiet NaN match would hide a non-value. Failing closed is the stricter report. |
| Integers longer than 4300 digits | `json.load` raises `ValueError: Exceeds the limit (4300 digits)` and writes no report | The integer is kept and compared; a report is written when the rest of the catalog can be read | Python's converter limit is not a catalog rule. The digits are still exact here. |
| A 64-hex digest with a trailing newline | `re` `$` matches before a final newline, so the digest passes the hex check | `$` does not match that newline, so the receipt is `no 64-hex sql_sha256` and the run fails | The newline is not a hex digit. Rejecting it keeps the digest check literal. |
| Non-integer float object names | Float names are ordered and compared | Ordering two non-integer floats, or a float with a string, raises `TypeError` and exits 1 | Integer-valued numbers compare; other floats have no single order here. One float name still compares. |
| A ledger row that is a list | Old-side `"lineage" in row` treats a list as a sequence and the report continues. A new-side list row raises `AttributeError` from `.get` | A non-object row exits 1. The old side says the list is not iterable; the new side says a list has no `get` | A receipt is a mapping. Continuing past a list would skip the lineage guard. |
| `seeds` is an empty list | `set([])` is empty, so there is no seed DIFF | A non-object `seeds` exits 1 with `TypeError: 'list' object is not iterable` | Seeds are a mapping of named facts. An empty object still compares. |
| `--report` spelling | Unique prefixes such as `--rep` and `--h` are accepted | Only `--report`, `--help`, and `-h` are options. `--rep` exits 2 as unrecognized. `-hx` does not open help | The documented flag is the contract. Prefix matching is an argparse quirk. |
| Input and usage failures | A CPython traceback ends with the exception line | stderr is that diagnosis line (or a short `JSONDecodeError` / `UnicodeDecodeError`) and the status is still 1 or 2 | The status and the failure are the contract. The traceback frames are not. |
