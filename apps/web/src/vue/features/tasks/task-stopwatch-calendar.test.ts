import { describe, expect, test } from "bun:test";
import { timerCalendarRange } from "./task-stopwatch-calendar";

describe("timer local calendar labels", () => {
  test("Seoul midnight moves the date while UTC is still the previous date", () => {
    expect(timerCalendarRange("2026-01-01T15:00:00.000Z", "Asia/Seoul", 1)).toEqual({
      today: "2026-01-02",
      weekFrom: "2025-12-29",
      weekTo: "2026-01-04",
    });
  });
  test("both DST overlap instants share a local date without changing elapsed policy", () => {
    const first = timerCalendarRange("2026-11-01T05:30:00Z", "America/New_York", 0);
    const second = timerCalendarRange("2026-11-01T06:30:00Z", "America/New_York", 0);
    expect(first).toEqual(second);
    expect(first).toEqual({ today: "2026-11-01", weekFrom: "2026-11-01", weekTo: "2026-11-07" });
  });
  test("week start follows the existing user preference", () => {
    expect(timerCalendarRange("2026-03-08T07:30:00Z", "America/New_York", 1).weekFrom).toBe(
      "2026-03-02",
    );
    expect(timerCalendarRange("2026-03-08T07:30:00Z", "America/New_York", 0).weekFrom).toBe(
      "2026-03-08",
    );
  });
});
