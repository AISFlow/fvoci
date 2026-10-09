import { Buffer } from "node:buffer";
import { stdout } from "node:process";
import type {
  FullConfig,
  FullResult,
  Reporter,
  Suite,
  TestCase,
  TestResult,
} from "@playwright/test/reporter";
import { z } from "zod";
import {
  BROWSER_DIAGNOSTIC_NAME,
  DIAGNOSTIC_NAME,
  timeSchema,
} from "../../../../tools/diagnostics/typeddiagnostic-schema";
import {
  asciiJson,
  browserDigest,
  diagnosticFromBody,
  unavailable,
} from "../../../../tools/diagnostics/trace-summary";

type DiagnosticCase = Pick<TestCase, "id" | "location">;
type DiagnosticResult = Pick<TestResult, "attachments" | "status" | "duration" | "retry">;
const statusSchema = z.enum(["passed", "failed", "timedOut", "skipped", "interrupted"]);
const HEADER = "browser summary: explicit diagnostic allowlist; private trace retained locally";

export function attachmentDiagnostic(
  attachments: TestResult["attachments"],
  name = DIAGNOSTIC_NAME,
) {
  if (attachments.length > 32) return unavailable("attachment_limit");
  const named = attachments.filter((value) => value.name === name);
  if (named.length !== 1) return unavailable("missing_or_duplicate_attachment");
  const attachment = named[0];
  if (!attachment || attachment.contentType !== "application/json" || attachment.path !== undefined)
    return unavailable("invalid_attachment_reference");
  if (!Buffer.isBuffer(attachment.body)) return unavailable("missing_or_invalid_attachment_member");
  if (name === DIAGNOSTIC_NAME) return diagnosticFromBody(attachment.body);
  if (attachment.body.length > 1024 * 1024) return unavailable("attachment_size_limit");
  try {
    const input: unknown = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(attachment.body),
    );
    return browserDigest(input);
  } catch {
    return unavailable("invalid_attachment_data");
  }
}

/** Direct Playwright lifecycle adapter; never opens a trace ZIP or emits raw stdio/errors. */
export default class DiagnosticReporter implements Reporter {
  private sources = new Map<string, string>();
  private completed = new Set<string>();
  private invalid = false;
  private emit: (line: string) => void;

  constructor(options: { emit?: (line: string) => void } = {}) {
    this.emit =
      options.emit ??
      ((line) => {
        stdout.write(line + "\n");
      });
  }
  begin(tests: readonly DiagnosticCase[]) {
    this.sources.clear();
    this.completed.clear();
    this.invalid = tests.length === 0;
    for (const test of tests) {
      if (!test.id || !test.location.file || this.sources.has(test.id)) this.invalid = true;
      this.sources.set(test.id, test.location.file);
    }
    this.emit(HEADER);
  }
  onBegin(_config: FullConfig, suite: Suite) {
    this.begin(suite.allTests());
  }
  onTestEnd(test: DiagnosticCase, result: DiagnosticResult) {
    if (
      !this.sources.has(test.id) ||
      this.sources.get(test.id) !== test.location.file ||
      this.completed.has(test.id)
    ) {
      this.invalid = true;
      this.emit(asciiJson(unavailable("foreign_report")));
      return;
    }
    const status = statusSchema.safeParse(result.status),
      duration = timeSchema().safeParse(result.duration);
    if (!status.success || !duration.success || result.retry !== 0) {
      this.invalid = true;
      this.emit(asciiJson(unavailable("invalid_report")));
      return;
    }
    this.completed.add(test.id);
    this.emit(
      asciiJson({
        status: status.data,
        duration: duration.data,
        exit: status.data === "passed" ? 0 : 1,
      }),
    );
    this.emit("w3-template-diagnostic " + asciiJson(attachmentDiagnostic(result.attachments)));
    if (result.attachments.some((value) => value.name === BROWSER_DIAGNOSTIC_NAME))
      this.emit(
        "browser-diagnostic " +
          asciiJson(attachmentDiagnostic(result.attachments, BROWSER_DIAGNOSTIC_NAME)),
      );
  }
  onError() {
    this.invalid = true;
  }
  onStdOut() {
    /* Raw worker output has no public projection. */
  }
  onStdErr() {
    /* Raw worker errors have no public projection. */
  }
  onEnd(result: Pick<FullResult, "status" | "duration">) {
    const valid =
      z.enum(["passed", "failed", "timedout", "interrupted"]).safeParse(result.status).success &&
      timeSchema().safeParse(result.duration).success;
    if (
      !valid ||
      this.invalid ||
      this.completed.size !== this.sources.size ||
      this.sources.size === 0
    ) {
      this.emit(asciiJson(unavailable("invalid_report")));
      return Promise.resolve({ status: "failed" as const });
    }
    return Promise.resolve({ status: result.status });
  }
  printsToStdio() {
    return true;
  }
}
