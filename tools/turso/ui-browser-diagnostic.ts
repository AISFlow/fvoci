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
        : /^(?:Error: )?No tests found(?:\.|$)/.test(message)
          ? "no-tests"
          : /^SyntaxError:/.test(message)
            ? "syntax"
            : status === "interrupted"
              ? "interrupted"
              : "other";

export const browserDiagnosticHelpers = ["collab-helpers.ts", "selected-final-native.ts"] as const;
export type BrowserDiagnosticSources = Readonly<Record<string, { path: string; source: string }>>;

export function browserFailureDiagnostic(
  report: unknown,
  spec: string,
  stage: "on" | "restart" | "off",
  sources: BrowserDiagnosticSources,
): Record_ {
  const source = sources[spec]?.source ?? "";
  const titles = new Set([...source.matchAll(/^(?: {2})?test\("([^"\n]+)"/gm)].map((m) => m[1]));
  const admitted = [spec, ...browserDiagnosticHelpers].flatMap((name) => {
    const item = sources[name];
    return item
      ? [
          {
            name,
            path: item.path,
            file: "apps/web/e2e-pending/" + name,
            lines: item.source.split("\n"),
          },
        ]
      : [];
  });
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
          // Playwright preserves TestError.location; errorLocation is only a
          // stack-derived fallback. An unqualified canonical location must not
          // be replaced by a different, apparently admitted stack location.
          const location = plain(
            Object.hasOwn(error, "location") ? error.location : result.errorLocation,
          );
          const located = admitted.find(
            (item) =>
              location.file === item.name ||
              location.file === item.file ||
              location.file === item.path,
          );
          const line =
            typeof location.line === "number" &&
            Number.isInteger(location.line) &&
            location.line > 0 &&
            located !== undefined &&
            location.line <= located.lines.length
              ? location.line
              : null;
          // A numeric Received value is an HTTP status only at a real status()
          // assertion in the admitted source; arbitrary body numbers stay private.
          const received = /^Received:\s+([1-5][0-9]{2})\s*$/m.exec(message);
          const httpStatus =
            line !== null &&
            /\.status\(\)\)\.toBe\([1-5][0-9]{2}\)/.test(located?.lines[line - 1] ?? "") &&
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
            file: line !== null && located ? located.file : null,
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
