import { afterEach, describe, expect, it } from "vitest";

import { setDisplayTimeZone } from "@/lib/format-date";
import { clockTime, dayLabel, listTime } from "@/lib/chat/layout";

// (P5-REFINE-20 batch 5 item 5, b2#18) One zone on a screen: the person's, for stamps, list times and day dividers alike.
afterEach(() => setDisplayTimeZone(null));

describe("chat times are in the person's zone", () => {
  const at = "2026-11-21T23:30:00Z";
  it("the clock", () => {
    setDisplayTimeZone("UTC");
    expect(clockTime(at)).toBe("23:30");
    setDisplayTimeZone("Asia/Tokyo");
    expect(clockTime(at)).toBe("08:30");
  });
  it("the list time and the day divider use that zone's calendar day", () => {
    setDisplayTimeZone("Asia/Tokyo");
    // 23:30 UTC on the 21st is the 22nd in Tokyo: "now" 10:00 UTC on the 22nd is the same Tokyo day.
    expect(listTime(at, new Date("2026-11-22T01:00:00Z"))).toBe("08:30");
    expect(dayLabel(at, new Date("2026-11-22T01:00:00Z"))).toBe("Today");
    setDisplayTimeZone("UTC");
    expect(dayLabel(at, new Date("2026-11-22T01:00:00Z"))).toBe("Yesterday");
  });
});
