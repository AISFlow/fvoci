// Public projection of the private Playwright JSON report. No report strings
// are copied except test titles registered in the admitted spec's source.
import { stripVTControlCharacters } from "node:util";
import { isRecord, type Record_ } from "./ui-common.ts";

const array = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const plain = (value: unknown): Record_ => (isRecord(value) ? value : {});
const stripAnsi = (value: unknown): string =>
  typeof value === "string" ? stripVTControlCharacters(value) : "";
const apiStep = (message: string): string | null =>
  /^(?:TimeoutError: |Error: )?(locator\.(?:click|fill|press|waitFor)|page\.(?:goto|waitForResponse)|apiRequestContext\.(?:get|post|put|delete)|browserType\.launch):/.exec(
    message,
  )?.[1] ?? null;
const categoryOf = (message: string, status?: unknown): string =>
  status === "timedOut" || /^(?:TimeoutError:|Test timeout of [0-9]+ms exceeded)/.test(message)
    ? "timeout"
    : /^Error: expect\(/.test(message)
      ? "assertion"
      : apiStep(message) === "browserType.launch"
        ? "browser-launch"
        : /^SyntaxError:/.test(message)
          ? "syntax"
          : status === "interrupted"
            ? "interrupted"
            : "other";

export function browserFailureDiagnostic(
  report: unknown,
  spec: string,
  stage: "on" | "restart" | "off",
  source: string,
): Record_ {
  const titles = new Set([...source.matchAll(/^(?: {2})?test\("([^"\n]+)"/gm)].map((m) => m[1]));
  const lines = source.split("\n");
  const failures: Record_[] = [];
  const pending = [...array(plain(report).suites)];
  let visited = 0;
  while (pending.length && visited++ < 512 && failures.length < 16) {
    const suite = plain(pending.pop());
    pending.push(...array(suite.suites).slice(0, 512));
    for (const entry of array(suite.specs).slice(0, 128)) {
      const item = plain(entry);
      for (const test of array(item.tests).slice(0, 16)) {
        for (const value of array(plain(test).results).slice(0, 16)) {
          const result = plain(value);
          if (!["failed", "timedOut", "interrupted"].includes(String(result.status))) continue;
          if (failures.length === 16) break;
          const error = plain(result.error);
          const message = stripAnsi(error.message);
          const location = plain(result.errorLocation);
          const line =
            typeof location.line === "number" &&
            Number.isInteger(location.line) &&
            location.line > 0 &&
            location.line <= lines.length &&
            typeof location.file === "string" &&
            (location.file === spec || location.file.endsWith("/e2e-pending/" + spec))
              ? location.line
              : null;
          // A numeric Received value is an HTTP status only at a real status()
          // assertion in the admitted source; arbitrary body numbers stay private.
          const received = /^Received:\s+([1-5][0-9]{2})\s*$/m.exec(message);
          const httpStatus =
            line !== null &&
            /\.status\(\)\)\.toBe\([1-5][0-9]{2}\)/.test(lines[line - 1] ?? "") &&
            received
              ? Number(received[1])
              : null;
          const category = categoryOf(message, result.status);
          failures.push({
            title:
              typeof item.title === "string" && titles.has(item.title)
                ? item.title
                : "unregistered",
            category,
            step:
              httpStatus !== null
                ? "http-status"
                : (apiStep(message) ?? (category === "assertion" ? "expect" : "test-body")),
            httpStatus,
            line,
          });
        }
      }
    }
  }
  return {
    originalFailure: "UI_ACTUAL_BROWSER_FAILED",
    browserFailure: {
      stage,
      report: isRecord(report) && Array.isArray(report.suites) ? "available" : "unavailable",
      failures,
      globalErrors: Math.min(array(plain(report).errors).length, 512),
      globalErrorCategories: array(plain(report).errors)
        .slice(0, 16)
        .map((error) => categoryOf(stripAnsi(plain(error).message))),
    },
  };
}
