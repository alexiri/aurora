import { describe, expect, it } from "vitest";

import { formatExact, formatWhen } from "@/lib/format-date";

const now = new Date(2026, 10, 19, 12, 0, 0).getTime();
const ago = (ms: number) => new Date(now - ms).toISOString();

describe("formatWhen", () => {
  it("says how long ago for the last day, with no seconds", () => {
    expect(formatWhen(ago(10_000), { now })).toBe("Just now");
    expect(formatWhen(ago(5 * 60_000), { now })).toBe("5 min ago");
    expect(formatWhen(ago(2 * 3_600_000), { now })).toBe("2 h ago");
  });
  it("says Yesterday for the previous calendar day, a short date after that, with the year only when it differs", () => {
    expect(formatWhen(new Date(2026, 10, 18, 9, 0).toISOString(), { now })).toBe("Yesterday");
    expect(formatWhen(new Date(2026, 10, 10, 9, 0).toISOString(), { now })).toMatch(/10/);
    expect(formatWhen(new Date(2026, 10, 10, 9, 0).toISOString(), { now })).not.toMatch(/2026/);
    expect(formatWhen(new Date(2025, 10, 10, 9, 0).toISOString(), { now })).toMatch(/2025/);
  });
  it("gives a dash (or the given word) for nothing or an invalid time", () => {
    expect(formatWhen(null)).toBe("—");
    expect(formatWhen("nope")).toBe("—");
    expect(formatWhen(undefined, { empty: "Never" })).toBe("Never");
  });
});

describe("formatExact", () => {
  it("is the locale date and time to the minute", () => {
    const s = formatExact(new Date(2026, 10, 19, 9, 5, 42).toISOString());
    expect(s).toMatch(/2026/);
    expect(s).not.toMatch(/:42/);
    expect(formatExact(null)).toBe("—");
  });
});
