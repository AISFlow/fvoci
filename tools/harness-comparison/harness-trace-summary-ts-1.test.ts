import { expect, test } from "bun:test";
import { Buffer } from "node:buffer";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Zip, ZipPassThrough, strToU8 } from "fflate";
import { z } from "zod";
import { diagnosticDigest, diagnosticFromBody, browserDigest } from "../diagnostics/trace-summary";
import {
  fixture,
  schemaCases,
  SENTINELS,
  browserFixture,
} from "../diagnostics/trace-summary-fixtures";
import { observerFixture } from "../diagnostics/template-observer-fixture";
import { DIAGNOSTIC_NAME } from "../diagnostics/typeddiagnostic-schema";
import { attachmentDiagnostic } from "../../apps/web/e2e/reporters/diagnostic-reporter";

const ROOT = resolve(import.meta.dir, "../..");
const EVIDENCE = join(ROOT, "target/trace-summary-ts-1");
const member = "attachments/" + "a".repeat(40);
const reference = { name: DIAGNOSTIC_NAME, contentType: "application/json", file: member };
const network = {
  type: "resource-snapshot",
  snapshot: {
    request: { method: "GET", url: "http://localhost/assets/fixture.js" },
    response: { status: 200 },
    _resourceType: "script",
    time: 1,
    _monotonicTime: 1,
  },
};
type Entry = readonly [string, string | Uint8Array];
function zip(entries: readonly Entry[]) {
  const chunks: Uint8Array[] = [];
  const archive = new Zip((error, chunk) => {
    if (error) throw error;
    chunks.push(chunk);
  });
  for (const [name, value] of entries) {
    const entry = new ZipPassThrough(name);
    archive.add(entry);
    entry.push(typeof value === "string" ? strToU8(value) : value, true);
  }
  archive.end();
  return Buffer.concat(chunks);
}
function legacy(entries: readonly Entry[]) {
  mkdirSync(EVIDENCE, { recursive: true });
  const work = mkdtempSync(join(EVIDENCE, "comparison-"));
  try {
    const path = join(work, "trace.zip");
    writeFileSync(path, zip(entries));
    // Only the existing Python script executes. Missing Python/module errors fail the test.
    const output = execFileSync("python3", [join(ROOT, "scripts/web-e2e-trace-summary.py"), path], {
      encoding: "utf8",
      timeout: 20000,
    });
    expect(output).toContain("GET 200 script /assets/fixture.js");
    for (const sentinel of SENTINELS) expect(output.includes(sentinel)).toBe(false);
    const records = output.split("\n").filter((line) => line.startsWith("w3-template-diagnostic "));
    expect(records).toHaveLength(1);
    const record = records[0];
    if (!record) throw new Error("Missing legacy diagnostic");
    const digest: unknown = JSON.parse(record.slice("w3-template-diagnostic ".length));
    expect(Buffer.byteLength(JSON.stringify(digest))).toBeLessThanOrEqual(49152);
    return digest;
  } finally {
    rmSync(work, { recursive: true });
  }
}
function entries(input: unknown, ref: unknown = reference, event?: string): Entry[] {
  return [
    ["0-trace.network", JSON.stringify(network)],
    ["test.trace", event ?? JSON.stringify({ type: "after", attachments: [ref] })],
    [
      member,
      typeof input === "string" || input instanceof Uint8Array ? input : JSON.stringify(input),
    ],
  ];
}
test("legacy comparator Python3 availability is required", () => {
  expect(execFileSync("python3", ["--version"], { encoding: "utf8" })).toMatch(/^Python 3\./);
});
for (const scenario of schemaCases)
  test(`legacy equivalence: ${scenario.name}`, () => {
    const old = legacy(entries(scenario.input)),
      current = diagnosticDigest(scenario.input);
    expect(old).toEqual(current);
    if (scenario.reason) expect(current).toEqual({ available: false, reason: scenario.reason });
    else expect(current.available).toBe(true);
    for (const sentinel of SENTINELS)
      expect(JSON.stringify(current).includes(sentinel)).toBe(false);
  });
for (const [name, value, reason] of [
  ["malformed", "{DIAGNOSTIC_PRIVATE_VALUE", "invalid_attachment_data"],
  ["oversize", new Uint8Array(1024 * 1024 + 1).fill(32), "attachment_size_limit"],
  [
    "non_json_number",
    JSON.stringify(fixture()).replace('"at":10', '"at":NaN'),
    "invalid_attachment_data",
  ],
] as const)
  test(`legacy equivalence: diagnostic reject ${name}`, () => {
    const body = typeof value === "string" ? strToU8(value) : value;
    expect(legacy(entries(value))).toEqual(diagnosticFromBody(body));
    expect(diagnosticFromBody(body)).toEqual({ available: false, reason });
  });
for (const [name, oldEntries, attachments, oldReason, newReason] of [
  [
    "path",
    entries(fixture(), { ...reference, file: "../DIAGNOSTIC_PRIVATE_VALUE" }),
    [
      {
        name: DIAGNOSTIC_NAME,
        contentType: "application/json",
        path: "../DIAGNOSTIC_PRIVATE_VALUE",
      },
    ],
    "invalid_attachment_reference",
    "invalid_attachment_reference",
  ],
  [
    "content_type",
    entries(fixture(), { ...reference, contentType: "text/plain" }),
    [
      {
        name: DIAGNOSTIC_NAME,
        contentType: "text/plain",
        body: Buffer.from(JSON.stringify(fixture())),
      },
    ],
    "invalid_attachment_reference",
    "invalid_attachment_reference",
  ],
  [
    "missing",
    entries(fixture(), { ...reference, file: "attachments/" + "b".repeat(40) }),
    [{ name: DIAGNOSTIC_NAME, contentType: "application/json" }],
    "missing_or_invalid_attachment_member",
    "missing_or_invalid_attachment_member",
  ],
  [
    "duplicate",
    [...entries(fixture()), [member, JSON.stringify(fixture())] as Entry],
    Array.from({ length: 2 }, () => ({
      name: DIAGNOSTIC_NAME,
      contentType: "application/json",
      body: Buffer.from(JSON.stringify(fixture())),
    })),
    "missing_or_invalid_attachment_member",
    "missing_or_duplicate_attachment",
  ],
  [
    "no_reference",
    entries(fixture(), undefined, "{}"),
    [],
    "missing_or_duplicate_attachment",
    "missing_or_duplicate_attachment",
  ],
  [
    "member_limit",
    [
      ...entries(fixture()),
      ...Array.from({ length: 4096 }, (_, i): Entry => [`unrelated/${String(i)}`, ""]),
    ],
    Array.from({ length: 33 }, () => ({
      name: DIAGNOSTIC_NAME,
      contentType: "application/json",
      body: Buffer.from("{}"),
    })),
    "member_limit",
    "attachment_limit",
  ],
] as const)
  test(`legacy refusal semantics: diagnostic reject ${name}`, () => {
    expect(legacy(oldEntries)).toEqual({ available: false, reason: oldReason });
    expect(attachmentDiagnostic([...attachments])).toEqual({ available: false, reason: newReason });
  });
test("legacy refusal semantics: diagnostic reject events / direct report event bound", () => {
  expect(
    legacy(entries(fixture(), reference, Array.from({ length: 10001 }, () => "{}").join("\n"))),
  ).toEqual({ available: false, reason: "test_event_limit" });
  expect(
    browserDigest({
      source: "fvoci-playwright",
      events: Array.from({ length: 10001 }, () => ({ kind: "crash", at: 1 })),
    }),
  ).toEqual({ available: false, reason: "invalid_report" });
});
test("legacy refusal semantics: diagnostic reject bad_test / invalid direct report", () => {
  expect(legacy(entries(fixture(), reference, "{DIAGNOSTIC_PRIVATE_VALUE"))).toEqual({
    available: false,
    reason: "invalid_attachment_data",
  });
  expect(browserDigest("{DIAGNOSTIC_PRIVATE_VALUE")).toEqual({
    available: false,
    reason: "invalid_report",
  });
});
test("legacy equivalence: observer safe reporter integration", () => {
  const raw = observerFixture(),
    current = diagnosticDigest(raw);
  expect(current.available).toBe(true);
  expect(legacy(entries(raw))).toEqual(current);
});
test("legacy equivalence: safe trace basic diagnostics route/status/privacy", () => {
  mkdirSync(EVIDENCE, { recursive: true });
  const work = mkdtempSync(join(EVIDENCE, "browser-"));
  try {
    const rows = browserFixture.events;
    const networkRows = rows
      .filter((value) => value.kind === "request")
      .map((value) => ({
        type: "resource-snapshot",
        snapshot: {
          request: { method: value.method, url: value.url },
          response: { status: value.status, _failureText: value.failure ?? "" },
          _resourceType: value.resourceType,
          startedDateTime: "2026-09-29T00:00:00.000Z",
          time: value.duration,
          _monotonicTime: value.at,
        },
      }));
    const rawConsole = browserFixture.events.find((value) => value.kind === "console");
    const rawError = browserFixture.events.find((value) => value.kind === "pageerror");
    const events = [
      {
        type: "console",
        messageType: "error",
        time: 2,
        text: rawConsole?.text,
        location: { url: "http://127.0.0.1:4000/assets/app.js" },
      },
      {
        type: "event",
        method: "pageError",
        time: 3,
        params: { error: { error: { name: "Error", message: rawError?.text } } },
      },
    ];
    const path = join(work, "trace.zip");
    writeFileSync(
      path,
      zip([
        ["0-trace.network", networkRows.map((value) => JSON.stringify(value)).join("\n")],
        ["0-trace.trace", events.map((value) => JSON.stringify(value)).join("\n")],
      ]),
    );
    const old = execFileSync("python3", [join(ROOT, "scripts/web-e2e-trace-summary.py"), path], {
      encoding: "utf8",
      timeout: 20000,
    });
    const current = browserDigest(browserFixture);
    if (!current.available) throw new Error("Browser diagnostic unavailable");
    const publicText = JSON.stringify(current);
    for (const sentinel of [
      "PATHSECRET",
      "QUERYSECRET",
      "PARAMSECRET1",
      "PARAMSECRET2",
      "PARAMSECRET3",
      "DBSECRET",
      "S".repeat(20),
      "B".repeat(20),
    ]) {
      expect(old.includes(sentinel)).toBe(false);
      expect(publicText.includes(sentinel)).toBe(false);
    }
    expect(old.startsWith("browser summary: ")).toBe(true);
    for (const row of current.rows) {
      if ("route" in row)
        expect(old).toContain(
          `${String(row.method)} ${String(row.status)} ${String(row.resourceType)} ${String(row.route)}`,
        );
      else if (row.kind === "console") expect(old).toContain("console   error fetch failed");
      else if (row.kind === "pageerror") expect(old).toContain("pageerror");
    }
    expect(publicText).toContain("net::ERR_ABORTED");
    expect(old).toContain("net::ERR_ABORTED");
  } finally {
    rmSync(work, { recursive: true });
  }
});
test("existing complete legacy fixture subprocess retains original negative/observer controls", () => {
  const output = execFileSync(
    "bash",
    [join(ROOT, "scripts/fixtures/web-e2e/trace-summary-fixture-test.sh")],
    { cwd: ROOT, encoding: "utf8" },
  );
  for (const line of [
    "trace-summary-fixture-test: ok",
    "15 typed/shape/overflow rejection controls PASS",
    "14 rejection controls PASS",
    "template observer pure fixture: remount/current-owner, gap/destroyed, scoped counters and cleanup PASS",
  ])
    expect(output).toContain(line);
  expect(z.string().parse(output).includes("DIAGNOSTIC_PRIVATE_VALUE")).toBe(false);
});
