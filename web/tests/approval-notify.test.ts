import { describe, expect, it } from "vitest";

import { noticeUrl, notificationParts, shouldNotify } from "@/lib/approval-notify";

const notice = {
  id: "a1",
  kind: "gateway",
  title: "Deploy to prod",
  expires_at: "2026-11-14T10:00:00Z",
  agent: { id: "g", name: "Daneel", handle: "daneel" },
  workspace: { id: "w", name: "Main", slug: "main", org_slug: "acme" },
};

const base = {
  event: "created",
  notice,
  enabled: true,
  permission: "granted" as const,
  hidden: true,
  focused: false,
};

describe("approval browser notifications (RFE-011a)", () => {
  it("notifies only a created approval, opted in, permitted, and while hidden or unfocused", () => {
    expect(shouldNotify(base)).toBe(true);
    expect(shouldNotify({ ...base, hidden: false, focused: false })).toBe(true);
    expect(shouldNotify({ ...base, hidden: false, focused: true })).toBe(false);
    expect(shouldNotify({ ...base, event: "decided" })).toBe(false);
    expect(shouldNotify({ ...base, notice: null })).toBe(false);
    expect(shouldNotify({ ...base, enabled: false })).toBe(false);
    expect(shouldNotify({ ...base, permission: "default" })).toBe(false);
    expect(shouldNotify({ ...base, permission: "denied" })).toBe(false);
    expect(shouldNotify({ ...base, permission: "unsupported" })).toBe(false);
  });

  it("carries agent, workspace and title, tags by approval id and links to the approvals page", () => {
    const p = notificationParts(notice);
    expect(p.options.tag).toBe("a1");
    expect(p.options.body).toContain("Daneel");
    expect(p.options.body).toContain("Main");
    expect(p.options.body).toContain("Deploy to prod");
    expect(noticeUrl(notice)).toBe("/o/acme/w/main/approvals?focus=a1");
  });
});
