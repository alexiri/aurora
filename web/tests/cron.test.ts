import { describe, expect, it } from "vitest";

import { cadenceWords, cronToSpec, isTooFrequent, minGapMinutes, PRESETS, specToCron, whenWords, zoneCity, type ScheduleSpec } from "@/lib/cron";

const spec = (p: Partial<ScheduleSpec>): ScheduleSpec => ({ preset: "daily", minute: 0, hour: 9, weekday: 1, ...p });

describe("presets", () => {
  it("there is no preset shorter than five minutes", () => {
    expect(PRESETS.map((p) => p.label)).toEqual(["Every 5 minutes", "Hourly", "Every day", "Weekdays", "Weekly", "Custom"]);
  });

  it.each([
    [spec({ preset: "every5" }), "*/5 * * * *"],
    [spec({ preset: "hourly", minute: 15 }), "15 * * * *"],
    [spec({ preset: "daily" }), "0 9 * * *"],
    [spec({ preset: "daily", hour: 23, minute: 45 }), "45 23 * * *"],
    [spec({ preset: "weekdays", hour: 8, minute: 30 }), "30 8 * * 1-5"],
    [spec({ preset: "weekly", weekday: 0 }), "0 9 * * 0"],
    [spec({ preset: "weekly", weekday: 6, hour: 18 }), "0 18 * * 6"],
  ])("%j writes %s and reads back", (s, cron) => {
    expect(specToCron(s)).toBe(cron);
    const back = cronToSpec(cron);
    expect(back.preset).toBe(s.preset);
    if (s.preset !== "every5") expect(back.minute).toBe(s.minute);
    if (s.preset === "daily" || s.preset === "weekdays" || s.preset === "weekly") expect(back.hour).toBe(s.hour);
    if (s.preset === "weekly") expect(back.weekday).toBe(s.weekday);
  });

  it("custom has no cron of its own", () => {
    expect(specToCron(spec({ preset: "custom" }))).toBeNull();
  });
});

describe("odd crons read as Custom", () => {
  it.each([
    "* * * * *",
    "*/10 * * * *",
    "*/15 9 * * 1-5",
    "0 9 1 * *",
    "0 9 * 6 *",
    "0 9 * * 1,3",
    "0 9 * * MON",
    "0,30 * * * *",
    "0 */2 * * *",
    "0 9-17 * * *",
    "60 * * * *",
    "0 24 * * *",
    "0 9 * * 7",
    "0 9 * *",
    "0 9 * * * *",
    "",
    "@daily",
    "5-10 * * * *",
    "0 9 * * 5-1",
    "x y z a b",
  ])("%j", (cron) => {
    expect(cronToSpec(cron).preset).toBe("custom");
    expect(cadenceWords(cron)).toBeNull();
    expect(whenWords(cron, "Europe/Zurich")).toBe("Custom schedule");
  });

  it("null and undefined are Custom too", () => {
    expect(cronToSpec(null).preset).toBe("custom");
    expect(cronToSpec(undefined).preset).toBe("custom");
  });
});

describe("sentences", () => {
  it.each([
    ["*/5 * * * *", "Zurich", "Every 5 minutes"],
    ["0 * * * *", "Zurich", "Every hour"],
    ["20 * * * *", "Zurich", "Every hour at minute 20"],
    ["0 9 * * *", "Europe/Zurich", "Every day at 09:00 · Zurich"],
    ["30 8 * * 1-5", "Europe/Zurich", "Every weekday at 08:30 · Zurich"],
    ["0 18 * * 5", "America/Argentina/Buenos_Aires", "Every Friday at 18:00 · Buenos Aires"],
    ["0 9 * * 0", "UTC", "Every Sunday at 09:00 · UTC"],
  ])("%s in %s", (cron, tz, words) => {
    expect(whenWords(cron, tz)).toBe(words);
  });

  it("zoneCity", () => {
    expect(zoneCity("Europe/Zurich")).toBe("Zurich");
    expect(zoneCity("UTC")).toBe("UTC");
    expect(zoneCity(null)).toBe("");
  });
});

describe("minGapMinutes and the Too often flag", () => {
  it.each([
    ["* * * * *", 1],
    ["*/1 * * * *", 1],
    ["*/2 * * * *", 2],
    ["*/4 * * * *", 4],
    ["*/5 * * * *", 5],
    ["*/7 * * * *", 4], // :56 to :00 of the next hour
    ["1,2 * * * *", 1],
    ["0,3 * * * *", 3],
    ["0,30 * * * *", 30],
    ["0,59 * * * *", 1],
    ["0,59 9 * * *", 59],
    ["0 * * * *", 60],
    ["10-14 * * * *", 1],
    ["10-30/4 * * * *", 4],
    ["0-59/5 * * * *", 5],
    ["30/5 * * * *", 5],
    ["0 9 * * *", 60],
    ["0,2 9-10 * * *", 2],
    ["*/3 9 * * 1-5", 3],
  ])("%s is %i", (cron, gap) => {
    expect(minGapMinutes(cron)).toBe(gap);
  });

  it.each(["", "0 9 * *", "a b c d e", "60 * * * *", "*/0 * * * *", "5-1 * * * *", "1,,2 * * * *", "1/2/3 * * * *"])("%j cannot be told", (cron) => {
    expect(minGapMinutes(cron)).toBeNull();
    expect(isTooFrequent(cron)).toBe(false);
  });

  it("flags only what is surely too often", () => {
    expect(isTooFrequent("* * * * *")).toBe(true);
    expect(isTooFrequent("*/4 * * * *")).toBe(true);
    expect(isTooFrequent("*/5 * * * *")).toBe(false);
    expect(isTooFrequent("0 9 * * *")).toBe(false);
    expect(isTooFrequent("1,2 * * * *")).toBe(true);
  });
});
