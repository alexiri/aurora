import { afterEach, describe, expect, it } from "vitest";

import { displayTimeZone, formatCount, formatDay, formatExact, formatTime, formatWhen, personTimeZone, setDisplayTimeZone, whenInSentence } from "@/lib/format-date";

const now = new Date(2026, 10, 19, 12, 0, 0).getTime();
const ago = (ms: number) => new Date(now - ms).toISOString();

afterEach(() => setDisplayTimeZone(null));

describe("formatWhen", () => {
  it("says how long ago for the last day, with no seconds", () => {
    expect(formatWhen(ago(10_000), { now })).toBe("Just now");
    expect(formatWhen(ago(5 * 60_000), { now })).toBe("5 min ago");
    expect(formatWhen(ago(2 * 3_600_000), { now })).toBe("2 h ago");
  });

  it("after a day is the dated form with the time: '18 Nov, 09:00', with the year only when it differs", () => {
    const same = formatWhen(new Date(2026, 10, 18, 9, 0).toISOString(), { now });
    expect(same).toMatch(/18/);
    expect(same).toMatch(/Nov/);
    expect(same).toMatch(/09:00|9:00/);
    expect(same).not.toMatch(/2026/);
    // "Yesterday" is dropped in favour of the dated form.
    expect(same).not.toMatch(/Yesterday/);
    const old = formatWhen(new Date(2025, 10, 10, 23, 50).toISOString(), { now });
    expect(old).toMatch(/2025/);
    expect(old).toMatch(/23:50|11:50/);
  });

  it("gives a dash (or the given word) for nothing or an invalid time", () => {
    expect(formatWhen(null)).toBe("—");
    expect(formatWhen("nope")).toBe("—");
    expect(formatWhen(undefined, { empty: "Never" })).toBe("Never");
  });

  it("a clock time is always 24-hour, whatever the locale, and can name its zone (item 18)", () => {
    const at = "2026-11-10T03:11:00Z";
    setDisplayTimeZone("UTC");
    expect(formatTime(at)).toBe("03:11");
    expect(formatTime(at, "", "America/Los_Angeles")).toBe("19:11");
    expect(formatExact(at)).not.toMatch(/AM|PM/i);
  });

  it("inside a sentence 'Just now' loses its capital ('last just now', 'Since just now'); other forms are unchanged", () => {
    expect(whenInSentence(new Date().toISOString())).toBe("just now");
    expect(whenInSentence(new Date(Date.now() - 5 * 60_000).toISOString())).toBe("5 min ago");
    expect(whenInSentence(null)).toBe("—");
  });

  it("there is one zone: the person's, else the browser's (b2#18)", () => {
    setDisplayTimeZone("Asia/Tokyo");
    expect(personTimeZone()).toBe("Asia/Tokyo");
    setDisplayTimeZone(null);
    expect(personTimeZone()).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  });

  it("shows times in the person's time zone", () => {
    const at = "2026-11-10T23:30:00Z";
    setDisplayTimeZone("Asia/Tokyo");
    expect(displayTimeZone()).toBe("Asia/Tokyo");
    expect(formatWhen(at, { now })).toMatch(/11 Nov|Nov 11/); // 08:30 the next day in Tokyo
    expect(formatTime(at)).toMatch(/08:30|8:30/);
    setDisplayTimeZone("America/Los_Angeles");
    expect(formatWhen(at, { now })).toMatch(/10 Nov|Nov 10/);
    setDisplayTimeZone("Not/AZone");
    expect(displayTimeZone()).toBeUndefined();
  });
});

describe("formatExact, formatDay, formatCount", () => {
  it("formatExact is the locale date and time to the minute", () => {
    const s = formatExact(new Date(2026, 10, 19, 9, 5, 42).toISOString());
    expect(s).toMatch(/2026/);
    expect(s).not.toMatch(/:42/);
    expect(formatExact(null)).toBe("—");
  });
  it("formatDay has no time", () => {
    const s = formatDay(new Date(2026, 10, 19, 9, 5).toISOString());
    expect(s).toMatch(/19/);
    expect(s).toMatch(/2026/);
    expect(s).not.toMatch(/:/);
  });
  it("formatCount groups digits", () => {
    expect(formatCount(1234567)).toMatch(/1.234.567|1,234,567/);
  });
});
