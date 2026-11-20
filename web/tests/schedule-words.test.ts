import { describe, expect, it } from "vitest";

import { clockIn, nextWords, runTimeWords } from "@/lib/schedule-words";

const NOW = new Date("2026-11-20T21:00:00Z"); // 22:00 in Zurich (CET, UTC+1)

describe("run times in the schedule's zone", () => {
  it("clockIn", () => {
    expect(clockIn("2026-11-20T22:55:00Z", "Europe/Zurich")).toBe("23:55");
    expect(clockIn("2026-11-20T22:55:00Z", "UTC")).toBe("22:55");
    expect(clockIn("2026-11-20T23:30:00Z", "Asia/Tokyo")).toBe("08:30");
    expect(clockIn("not a date", "UTC")).toBe("");
    expect(clockIn("2026-11-20T22:55:00Z", "Nowhere/Land")).toBe("");
  });

  it("runTimeWords: today, tomorrow, then the weekday and date", () => {
    expect(runTimeWords("2026-11-20T22:30:00Z", "Europe/Zurich", NOW)).toBe("today 23:30");
    expect(runTimeWords("2026-11-20T23:30:00Z", "Europe/Zurich", NOW)).toBe("tomorrow 00:30");
    expect(runTimeWords("2026-11-21T08:00:00Z", "Europe/Zurich", NOW)).toBe("tomorrow 09:00");
    expect(runTimeWords("2026-11-24T08:00:00Z", "Europe/Zurich", NOW)).toBe("Tue 24 Nov, 09:00");
  });

  it("nextWords: three clock times for the short cadences, one sentence with the zone for the rest", () => {
    const three = ["2026-11-20T22:55:00Z", "2026-11-20T23:00:00Z", "2026-11-20T23:05:00Z"];
    expect(nextWords("every5", three, "Europe/Zurich", NOW)).toBe("next 23:55, 00:00, 00:05.");
    expect(nextWords("daily", ["2026-11-21T08:00:00Z"], "Europe/Zurich", NOW)).toBe("next: tomorrow 09:00 (Zurich time).");
    expect(nextWords("daily", [], "Europe/Zurich", NOW)).toBe("");
  });
});
