import { describe, expect, it } from "vitest";

import { altitudeAllows, viewerFloor, visibleTopics, type ViewerRoles } from "@/lib/api/roles";

const member: ViewerRoles = { instanceAdmin: false, workspace: "member" };
const approver: ViewerRoles = { instanceAdmin: false, workspace: "approver" };
const teamOwner: ViewerRoles = { instanceAdmin: false, workspace: "owner" };
const personalOwner: ViewerRoles = { instanceAdmin: false, workspace: "owner", personalWorkspace: true };

describe("viewerFloor (D-406)", () => {
  it("starts members, approvers and the owner of a personal workspace at Use", () => {
    expect(viewerFloor(member)).toBe("use");
    expect(viewerFloor(approver)).toBe("use");
    expect(viewerFloor(personalOwner)).toBe("use");
    expect(viewerFloor({ instanceAdmin: false, org: "member" })).toBe("use");
  });
  it("starts team-workspace owners, organization owners and admins, and instance admins at Set up", () => {
    expect(viewerFloor(teamOwner)).toBe("setup");
    expect(viewerFloor({ instanceAdmin: false, org: "owner" })).toBe("setup");
    expect(viewerFloor({ instanceAdmin: false, org: "admin" })).toBe("setup");
    expect(viewerFloor({ instanceAdmin: true })).toBe("setup");
  });
  it("starts an instance admin at Expert inside Administration", () => {
    expect(viewerFloor({ instanceAdmin: true }, true)).toBe("expert");
    expect(viewerFloor(member, true)).toBe("use");
  });
});

describe("altitudeAllows", () => {
  it("shows what the floor reaches and everything with the switch on", () => {
    expect(altitudeAllows("use", "use", false)).toBe(true);
    expect(altitudeAllows("setup", "use", false)).toBe(false);
    expect(altitudeAllows("expert", "setup", false)).toBe(false);
    expect(altitudeAllows("setup", "setup", false)).toBe(true);
    expect(altitudeAllows("expert", "use", true)).toBe(true);
    expect(altitudeAllows("expert", "expert", false)).toBe(true);
  });
});

describe("visibleTopics by altitude", () => {
  it("lists Overview, Agents and Approvals only at Use (approvals where the role may approve)", () => {
    const use = { floor: "use", more: false } as const;
    expect(visibleTopics("workspace", member, use)).toEqual(["overview", "agents"]);
    expect(visibleTopics("workspace", approver, use)).toEqual(["overview", "agents", "approvals"]);
    // (D-506(b)) A workspace's owner, team or personal, also has Skills.
    expect(visibleTopics("workspace", personalOwner, use)).toEqual(["overview", "agents", "skills", "approvals"]);
  });
  it("lists the rest at Set up, with role caps kept", () => {
    const setup = { floor: "setup", more: false } as const;
    expect(visibleTopics("workspace", teamOwner, setup)).toEqual([
      "overview",
      "agents",
      "skills",
      "apps",
      "internet-access",
      "approvals",
      "activity",
      "people",
      "settings",
    ]);
  });
  it("a member with the switch on does not get owner-only topics", () => {
    const topics = visibleTopics("workspace", member, { floor: "use", more: true });
    expect(topics).toContain("activity");
    expect(topics).not.toContain("settings");
    expect(topics).not.toContain("people");
    expect(topics).not.toContain("approvals");
  });
  it("an organization member lists Overview only at Use, then AI models and Internet access", () => {
    const m: ViewerRoles = { instanceAdmin: false, org: "member" };
    expect(visibleTopics("org", m, { floor: "use", more: false })).toEqual(["overview"]);
    expect(visibleTopics("org", m, { floor: "use", more: true })).toEqual(["overview", "ai-models", "internet-access"]);
  });
  it("lists every Administration topic to an instance admin, and none to others", () => {
    expect(visibleTopics("admin", { instanceAdmin: true }, { floor: "expert", more: false })).toHaveLength(11);
    expect(visibleTopics("admin", member)).toEqual([]);
  });
  it("without an altitude lists what the role may see (unchanged)", () => {
    expect(visibleTopics("workspace", member)).toContain("skills");
  });
});
