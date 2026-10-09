import { expect, test } from "bun:test";
import { Buffer } from "node:buffer";
import DiagnosticReporter, { attachmentDiagnostic } from "./diagnostic-reporter";
import {
  DIAGNOSTIC_NAME,
  BROWSER_DIAGNOSTIC_NAME,
} from "../../../../tools/diagnostics/typeddiagnostic-schema";
import {
  fixture,
  browserFixture,
  SENTINELS,
} from "../../../../tools/diagnostics/trace-summary-fixtures";
import { observerFixture } from "../../../../tools/diagnostics/template-observer-fixture";
import { diagnosticDigest } from "../../../../tools/diagnostics/trace-summary";

const source = { id: "fixture-case", location: { file: "source.spec.ts", line: 1, column: 1 } };
const bodyAttachment = () => ({
  name: DIAGNOSTIC_NAME,
  contentType: "application/json",
  body: Buffer.from(JSON.stringify(fixture())),
});
const result = () => ({
  status: "passed" as const,
  duration: 12,
  retry: 0,
  attachments: [bodyAttachment()],
});
test("reporter exact public lifecycle/source and one bounded diagnostic", async () => {
  const output: string[] = [],
    reporter = new DiagnosticReporter({
      emit: (line) => {
        output.push(line);
      },
    });
  reporter.begin([source]);
  reporter.onTestEnd(source, {
    ...result(),
    attachments: [
      ...result().attachments,
      {
        name: BROWSER_DIAGNOSTIC_NAME,
        contentType: "application/json",
        body: Buffer.from(JSON.stringify(browserFixture)),
      },
    ],
  });
  expect(await reporter.onEnd({ status: "passed", duration: 12 })).toEqual({ status: "passed" });
  expect(output[0]?.startsWith("browser summary: ")).toBe(true);
  expect(output.filter((line) => line.startsWith("w3-template-diagnostic "))).toHaveLength(1);
  expect(output.filter((line) => line.startsWith("browser-diagnostic "))).toHaveLength(1);
  for (const sentinel of [
    ...SENTINELS,
    "fixture-case",
    "source.spec.ts",
    "QUERYSECRET",
    "DBSECRET",
  ])
    expect(output.join("\n").includes(sentinel)).toBe(false);
  expect(output[1]).toBe('{"status":"passed","duration":12,"exit":0}');
});
for (const [name, attachments, reason] of [
  [
    "path",
    [{ ...bodyAttachment(), path: "../DIAGNOSTIC_PRIVATE_VALUE" }],
    "invalid_attachment_reference",
  ],
  [
    "content_type",
    [{ ...bodyAttachment(), contentType: "text/plain" }],
    "invalid_attachment_reference",
  ],
  [
    "missing",
    [{ name: DIAGNOSTIC_NAME, contentType: "application/json" }],
    "missing_or_invalid_attachment_member",
  ],
  ["duplicate", [bodyAttachment(), bodyAttachment()], "missing_or_duplicate_attachment"],
  ["no_reference", [], "missing_or_duplicate_attachment"],
  ["member_limit", Array.from({ length: 33 }, bodyAttachment), "attachment_limit"],
] as const)
  test(`direct reporter reject ${name}`, () => {
    expect(attachmentDiagnostic([...attachments])).toEqual({ available: false, reason });
  });
for (const name of [
  "blank",
  "invalid",
  "foreign",
  "duplicate",
  "missing",
  "retry",
  "raw-error",
] as const)
  test(`reporter refuses ${name} report`, async () => {
    const output: string[] = [],
      reporter = new DiagnosticReporter({
        emit: (line) => {
          output.push(line);
        },
      });
    reporter.begin(name === "blank" ? [] : [source]);
    if (name !== "blank" && name !== "missing")
      reporter.onTestEnd(
        name === "foreign"
          ? { ...source, location: { ...source.location, file: "foreign.spec.ts" } }
          : source,
        { ...result(), duration: name === "invalid" ? NaN : 12, retry: name === "retry" ? 1 : 0 },
      );
    if (name === "duplicate") reporter.onTestEnd(source, result());
    if (name === "raw-error") reporter.onError();
    expect(await reporter.onEnd({ status: "passed", duration: 12 })).toEqual({ status: "failed" });
    for (const sentinel of SENTINELS) expect(output.join("\n").includes(sentinel)).toBe(false);
  });
test("source-free diagnostic cannot claim available PASS", () => {
  expect(
    attachmentDiagnostic([
      { name: "trace", contentType: "application/zip", path: "private-trace.zip" },
    ]),
  ).toEqual({ available: false, reason: "missing_or_duplicate_attachment" });
});
test("status and duration use fixed exit allowlist without altering run failures", async () => {
  for (const status of ["passed", "failed", "timedOut", "skipped", "interrupted"] as const) {
    const output: string[] = [],
      reporter = new DiagnosticReporter({
        emit: (line) => {
          output.push(line);
        },
      });
    reporter.begin([source]);
    reporter.onTestEnd(source, { ...result(), status });
    expect(output[1]).toBe(
      JSON.stringify({ status, duration: 12, exit: status === "passed" ? 0 : 1 }),
    );
    expect(await reporter.onEnd({ status: "failed", duration: 12 })).toEqual({ status: "failed" });
  }
  for (const duration of [-1, Infinity, 1e12 + 1]) {
    const reporter = new DiagnosticReporter({ emit: () => {} });
    reporter.begin([source]);
    reporter.onTestEnd(source, { ...result(), duration });
    expect(await reporter.onEnd({ status: "passed", duration: 12 })).toEqual({ status: "failed" });
  }
});
for (const row of [
  "observer owner replacement",
  "observer gap/destroyed",
  "observer remount counters",
  "observer bounded mismatch retention",
  "observer stop cleanup",
  "observer safe reporter integration",
])
  test(row, () => {
    // The fixture runs every original ownership/counter/cleanup assertion against current source.
    const raw = observerFixture(),
      digest = diagnosticDigest(raw);
    if (!digest.available) throw new Error("Observer diagnostic unavailable");
    expect(digest.firstRetiredEvent).toMatchObject({
      unavailable: "missing-editor",
      eventBindingGeneration: 1,
      bindingGeneration: 3,
      retiredEvent: true,
      auth: { authenticated: null, synced: null, scope: "unknown", status: "unknown" },
      owner: [null, null, null, null, null, null, null],
    });
    expect(digest.counts.critical?.unknown).toBe(false);
    expect(digest.counts.critical?.dropped).toBeGreaterThan(256);
    const output = JSON.stringify(digest);
    for (const sentinel of ["한글과", "😀", "pmDocument"])
      expect(output.includes(sentinel)).toBe(false);
  });
