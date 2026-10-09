import type { FullConfig } from "@playwright/test";
import type { FullResult, Reporter, Suite, TestCase, TestResult } from "@playwright/test/reporter";

// Only counts and timings are public. Test titles, paths, errors, stdout and
// attachments remain with the runner and are never emitted by this reporter.
export default class AcceptanceReporter implements Reporter {
  private selected = new Set<string>();
  private completed = new Set<string>();
  private failed = false;

  printsToStdio(): boolean {
    return true;
  }
  onBegin(config: FullConfig, suite: Suite): void {
    const tests = suite.allTests();
    this.selected = new Set(tests.map((test) => test.id));
    this.failed =
      tests.length === 0 ||
      this.selected.size !== tests.length ||
      tests.some((test) => test.expectedStatus !== "passed") ||
      config.projects.some((project) => project.retries !== 0 || project.repeatEach !== 1);
  }
  onTestEnd(test: TestCase, result: TestResult): void {
    if (
      !this.selected.has(test.id) ||
      this.completed.has(test.id) ||
      result.status !== "passed" ||
      result.retry !== 0
    )
      this.failed = true;
    this.completed.add(test.id);
  }
  onError(): void {
    this.failed = true;
  }
  onEnd(result: FullResult): Promise<{ status: FullResult["status"] }> {
    const passed =
      !this.failed &&
      result.status === "passed" &&
      this.selected.size > 0 &&
      this.completed.size === this.selected.size;
    console.log(
      "web-e2e: selected=" +
        String(this.selected.size) +
        " completed=" +
        String(this.completed.size) +
        " duration_ms=" +
        String(Math.ceil(result.duration)) +
        " status=" +
        (passed ? "passed" : "failed"),
    );
    return Promise.resolve({ status: passed ? "passed" : "failed" });
  }
}
