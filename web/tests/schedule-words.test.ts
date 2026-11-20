import { describe, expect, it } from "vitest";

import { clockIn, dayMonthIn, nextWords, onceInstantWords, onceLocalWords, runTimeWords, tomorrowIn } from "@/lib/schedule-words";

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

describe("one-time and yearly words (P5-SCHED-ONCE)", () => {
  it("nextWords for Every year gives the date with its year", () => {
    expect(nextWords("yearly", ["2027-11-27T09:00:00Z"], "Europe/Zurich", NOW)).toBe("next: 27 Nov 2027 (Zurich time).");
  });

  it("onceInstantWords and onceLocalWords: the weekday, the date and the time, no today or tomorrow", () => {
    expect(onceInstantWords("2026-11-21T08:00:00Z", "Europe/Zurich")).toBe("Sat 21 Nov at 09:00");
    expect(onceLocalWords("2026-11-21T09:00:00")).toBe("Sat 21 Nov at 09:00");
    expect(onceLocalWords("2026-11-21T09:00")).toBe("Sat 21 Nov at 09:00");
    expect(onceLocalWords("tomorrow")).toBe("");
  });

  it("dayMonthIn is the day in the zone", () => {
    expect(dayMonthIn("2026-12-20T23:30:00Z", "Europe/Zurich")).toBe("21 Dec");
  });

  it("tomorrowIn reads the zone's today, not UTC's", () => {
    expect(tomorrowIn("Europe/Zurich", NOW)).toBe("2026-11-21");
    // 23:30 UTC on the 20th is 00:30 on the 21st in Zurich: tomorrow there is the 22nd.
    const late = new Date("2026-11-20T23:30:00Z");
    expect(tomorrowIn("Europe/Zurich", late)).toBe("2026-11-22");
    expect(tomorrowIn("UTC", late)).toBe("2026-11-21");
  });
});
