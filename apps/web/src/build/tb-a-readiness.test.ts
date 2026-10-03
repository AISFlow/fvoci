import assert from "node:assert/strict";
import test from "node:test";
import { createRustReadinessReader } from "../../e2e-pending/tb-a-readiness.ts";

const address = "http://127.0.0.1:42525";
const line = `fvoci-server listening on ${address}\n`;

for (const stream of ["stdout", "stderr"] as const) {
  await test(`${stream}: every listening byte split stays pending until the literal full line`, () => {
    const bytes = Buffer.from(line);
    for (let split = 0; split < bytes.length; split++) {
      const resolved: string[] = [];
      const read = createRustReadinessReader((url) => resolved.push(url));
      read(stream, bytes.subarray(0, split));
      assert.deepEqual(resolved, [], `premature resolution at byte ${String(split)}`);
      read(stream, bytes.subarray(split));
      assert.deepEqual(resolved, [address]);
    }
  });
}

await test("actual truncated-address schedule resolves only after the remainder and newline", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  read("stderr", Buffer.from("fvoci-server listening on http://127"));
  assert.deepEqual(resolved, []);
  read("stderr", Buffer.from(".0.0.1:42525"));
  assert.deepEqual(resolved, []);
  read("stderr", Buffer.from("\n"));
  assert.deepEqual(resolved, [address]);
});

await test("stdout cannot supply the terminator or address tail of a pending stderr line", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  read("stderr", Buffer.from("fvoci-server listening on http://127"));
  read("stdout", Buffer.from(".0.0.1:42525\n"));
  assert.deepEqual(resolved, []);
  read("stderr", Buffer.from(".0.0.1:42525\n"));
  assert.deepEqual(resolved, [address]);
});

await test("a full address without newline remains unavailable even after unrelated complete logs", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  read("stderr", Buffer.from(line.slice(0, -1)));
  read("stdout", Buffer.from("normal startup diagnostic\n"));
  assert.deepEqual(resolved, []);
});

await test("one-byte UTF-8 diagnostics and CRLF do not alter the emitted full address", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  const bytes = Buffer.from(`준비 🧑‍💻\n${line.replace("\n", "\r\n")}`);
  for (const byte of bytes) read("stderr", Buffer.from([byte]));
  assert.deepEqual(resolved, [address]);
});

await test("multiple lines and streams publish only the first complete readiness result", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  read("stdout", Buffer.from(`startup\n${line}later log\n`));
  read("stderr", Buffer.from("fvoci-server listening on http://127.0.0.1:53211\n"));
  assert.deepEqual(resolved, [address]);
});

for (const invalid of [
  "http://127",
  "http://127.0.0.1",
  "http://127.0.0.1:0",
  "http://127.0.0.1:65536",
  "http://127.0.0.1:42525garbage",
  "http://127.0.0.1:42525/path",
  "http://127.0.0.1:42525?query",
  "http://127.0.0.1:42525#fragment",
  "http://127.0.0.1:0042",
  "https://127.0.0.1:42525",
  "http://localhost:42525",
  "http://user@127.0.0.1:42525",
  "http://127.0.0.1:42525 trailing words",
]) {
  await test(`incomplete or foreign listener ${invalid} cannot become this fixture's proxy target`, () => {
    const resolved: string[] = [];
    const read = createRustReadinessReader((url) => resolved.push(url));
    read("stderr", Buffer.from(`fvoci-server listening on ${invalid}\n`));
    assert.deepEqual(resolved, []);
    read("stderr", Buffer.from(line));
    assert.deepEqual(resolved, [address]);
  });
}

for (const port of [1, 80, 65535]) {
  await test(`entire bound address preserves legitimate port ${String(port)}`, () => {
    const expected = `http://127.0.0.1:${String(port)}`;
    const resolved: string[] = [];
    const read = createRustReadinessReader((url) => resolved.push(url));
    read("stderr", Buffer.from(`fvoci-server listening on ${expected}\n`));
    assert.deepEqual(resolved, [expected]);
  });
}

await test("the suffix of an oversized incomplete log is not treated as a new readiness line", () => {
  const resolved: string[] = [];
  const read = createRustReadinessReader((url) => resolved.push(url));
  read("stderr", Buffer.from("x".repeat(8193)));
  read("stderr", Buffer.from(line));
  assert.deepEqual(resolved, []);
  read("stderr", Buffer.from(line));
  assert.deepEqual(resolved, [address]);
});
