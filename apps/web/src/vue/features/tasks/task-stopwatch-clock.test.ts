import { describe, expect, test } from "bun:test";
import { anchoredElapsed, stopwatchText } from "./task-stopwatch-clock";

describe("server stopwatch display", () => {
  test("renders the server anchor independently of browser wall clock/DST", () => {
    expect(
      anchoredElapsed(750, "2026-11-01T01:59:30-04:00", "2026-11-01T01:00:30-05:00", 100, 350),
    ).toBe(61000);
    expect(stopwatchText(61000)).toBe("00:01:01");
  });
  test("paused/closed state excludes time since server sample", () => {
    expect(anchoredElapsed(1500, null, "2026-10-02T00:00:00Z", 0, 1e8)).toBe(1500);
    expect(stopwatchText(1500)).toBe("00:00:01");
  });
  test("monotonic clock regression cannot subtract elapsed or persist a new range", () => {
    expect(anchoredElapsed(250, "2026-10-02T00:00:00Z", "2026-10-02T00:00:01Z", 1000, 999)).toBe(
      1250,
    );
  });
});
