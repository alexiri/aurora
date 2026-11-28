import { afterEach, beforeEach, describe, expect, it } from "vitest";

// (FIX-236, D-478, C-1164/C-1165) The banner and history words, pinned sentence by sentence:
// the reader's banner, the people's banner active and ended, the span, the end labels.
import {
  EMERGENCY_END,
  emergencyCovers,
  isEmergencyReader,
  emergencyEndText,
  emergencyPeopleBannerText,
  emergencyReaderBannerText,
  emergencySpanText,
  emergencyVisibilityLine,
  type WorkspaceEmergency,
} from "@/lib/emergency";
import { setDisplayTimeZone } from "@/lib/format-date";

beforeEach(() => setDisplayTimeZone("UTC"));
afterEach(() => setDisplayTimeZone(null));

const grant = (over: Partial<WorkspaceEmergency> = {}): WorkspaceEmergency => ({
  id: "e1",
  by: { id: "u1", display_name: "Ada Lovelace" },
  reason: "Legal hold on the account's records",
  opened_at: "2026-11-20T14:00:00Z",
  expires_at: "2026-11-20T18:00:00Z",
  ended_at: null,
  active: true,
  ...over,
});

describe("emergencyReaderBannerText", () => {
  it("says until when, that everything is recorded and that the people have been told", () => {
    expect(emergencyReaderBannerText("2026-11-20T18:00:00Z")).toBe(
      "Emergency read access until 18:00. Everything you open is recorded, and this workspace's people have been told.",
    );
  });
});

describe("emergencyPeopleBannerText", () => {
  it("active: who has it, until when, and the reason", () => {
    expect(emergencyPeopleBannerText(grant())).toBe(
      "Ada Lovelace, an organization owner, has emergency read access to this workspace until 18:00. Reason: Legal hold on the account's records.",
    );
  });

  it("ended: who had it, from to, and the reason", () => {
    expect(emergencyPeopleBannerText(grant({ active: false, ended_at: "2026-11-20T16:00:00Z" }))).toBe(
      "Ada Lovelace, an organization owner, had emergency read access from 14:00 to 16:00. Reason: Legal hold on the account's records.",
    );
  });

  // (FIX-236, review LOW-11) An expired grant the minute sweep has not ended yet: `ended_at` is
  // still null, and the span must fall back to `expires_at` — never "from 14:00 to 14:00".
  it("expired but not yet swept: the span ends at expires_at, not at the open time", () => {
    expect(emergencyPeopleBannerText(grant({ active: false, ended_at: null }))).toBe(
      "Ada Lovelace, an organization owner, had emergency read access from 14:00 to 18:00. Reason: Legal hold on the account's records.",
    );
  });

  // (FIX-236, review HIGH-1) Two simultaneous openers are both announced: the newest active by
  // name, the other by count.
  it("active with another active grant: along with K more organization owners", () => {
    expect(emergencyPeopleBannerText(grant(), 1)).toBe(
      "Ada Lovelace, an organization owner, has emergency read access to this workspace until 18:00, along with 1 more organization owner. Reason: Legal hold on the account's records.",
    );
    expect(emergencyPeopleBannerText(grant(), 2)).toBe(
      "Ada Lovelace, an organization owner, has emergency read access to this workspace until 18:00, along with 2 more organization owners. Reason: Legal hold on the account's records.",
    );
  });

  it("a reason that already ends its sentence gets no second full stop", () => {
    expect(emergencyPeopleBannerText(grant({ reason: "Looking into a report (test cluster)." }))).toBe(
      "Ada Lovelace, an organization owner, has emergency read access to this workspace until 18:00. Reason: Looking into a report (test cluster).",
    );
    expect(emergencyPeopleBannerText(grant({ active: false, ended_at: "2026-11-20T16:00:00Z", reason: "Is the export complete? " }))).toBe(
      "Ada Lovelace, an organization owner, had emergency read access from 14:00 to 16:00. Reason: Is the export complete?",
    );
  });

  it("a name is never shown raw", () => {
    const text = emergencyPeopleBannerText(grant({ by: { id: "u1", display_name: "Ann <b> & Co" } }));
    expect(text).toContain("Ann <b> & Co");
    expect(text).not.toContain("&amp;");
  });
});

describe("emergencySpanText", () => {
  it("same day: the two clock times", () => {
    expect(emergencySpanText("2026-11-20T14:00:00Z", "2026-11-20T16:30:00Z")).toBe("14:00 to 16:30");
  });

  it("days differ: each time carries its day", () => {
    expect(emergencySpanText("2026-11-20T22:00:00Z", "2026-11-21T02:00:00Z")).toBe(
      "Nov 20, 2026, 22:00 to Nov 21, 2026, 02:00",
    );
  });
});

describe("emergencyVisibilityLine", () => {
  it("the chat's extra line", () => {
    expect(emergencyVisibilityLine(grant())).toBe(
      "Ada Lovelace, an organization owner, can read this workspace's conversations until 18:00 (emergency access).",
    );
  });
});

describe("emergencyEndText", () => {
  it("active: until when", () => {
    expect(emergencyEndText({ active: true, expires_at: "2026-11-20T18:00:00Z", end_reason: null })).toBe("Until 18:00");
  });

  it("ended: the label for every end reason", () => {
    expect(emergencyEndText({ active: false, expires_at: "2026-11-20T18:00:00Z", end_reason: "ended" })).toBe("Ended");
    expect(emergencyEndText({ active: false, expires_at: "2026-11-20T18:00:00Z", end_reason: "expired" })).toBe("Expired");
    expect(emergencyEndText({ active: false, expires_at: "2026-11-20T18:00:00Z", end_reason: "access_lost" })).toBe("Access lost");
    expect(Object.keys(EMERGENCY_END).sort()).toEqual(["access_lost", "ended", "expired"]);
  });
});

describe("what a reader may open (FIX-277)", () => {
  it("only the viewer's own grant makes them a reader", () => {
    expect(isEmergencyReader({ emergency_reader: true })).toBe(true);
    for (const w of [{ emergency_reader: false }, {}, null, undefined]) expect(isEmergencyReader(w)).toBe(false);
  });

  it.each(["", "agents", "agents/daneel", "activity", "skills", "people"])("covers /%s", (rest) => {
    expect(emergencyCovers(rest)).toBe(true);
  });

  // Anything not named is not covered, a page added later included.
  it.each(["settings", "apps", "apps/github", "apps/github/connect", "credentials", "internet-access", "approvals", "welcome", "agents/new/t1", "agents/daneel/terminal", "people/invite", "something-new"])(
    "doesn't cover /%s",
    (rest) => {
      expect(emergencyCovers(rest)).toBe(false);
    },
  );
});
