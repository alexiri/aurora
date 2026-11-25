import { afterEach, beforeEach, describe, expect, it } from "vitest";

// (FIX-236, D-478, C-1164/C-1165) The banner and history words, pinned sentence by sentence:
// the reader's banner, the people's banner active and ended, the span, the end labels.
import {
  EMERGENCY_END,
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
