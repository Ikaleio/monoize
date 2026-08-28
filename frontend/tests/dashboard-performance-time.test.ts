import { describe, expect, test } from "bun:test";
import {
  formatPerformanceBrickRange,
  performanceBrickRange,
} from "../src/lib/performance-time";

describe("performance brick time ranges (DH-9b and DH-9c)", () => {
  test("derives each brick from the exact response interval", () => {
    const first = performanceBrickRange(
      0,
      24,
      "2026-08-27T09:47:00.000Z",
      "2026-08-28T09:47:00.000Z",
    );
    const last = performanceBrickRange(
      23,
      24,
      "2026-08-27T09:47:00.000Z",
      "2026-08-28T09:47:00.000Z",
    );

    expect(first?.start.toISOString()).toBe("2026-08-27T09:47:00.000Z");
    expect(first?.end.toISOString()).toBe("2026-08-27T10:47:00.000Z");
    expect(last?.start.toISOString()).toBe("2026-08-28T08:47:00.000Z");
    expect(last?.end.toISOString()).toBe("2026-08-28T09:47:00.000Z");
  });

  test("rejects invalid response bounds and brick indexes", () => {
    expect(
      performanceBrickRange(24, 24, "2026-08-27", "2026-08-28"),
    ).toBeNull();
    expect(performanceBrickRange(0, 0, "2026-08-27", "2026-08-28")).toBeNull();
    expect(performanceBrickRange(0, 24, "bad", "2026-08-28")).toBeNull();
  });

  test("formats a real localized interval instead of an ordinal hour", () => {
    const formatted = formatPerformanceBrickRange(
      0,
      24,
      "2026-08-27T09:47:00.000Z",
      "2026-08-28T09:47:00.000Z",
      "en-US",
    );

    expect(formatted).not.toBeNull();
    expect(formatted).not.toMatch(/^h\d+$/);
    expect(formatted).toContain("Aug");
  });
});
