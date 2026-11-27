import { describe, expect, it } from "vitest";

import { visibleTopics } from "@/lib/api/roles";

describe("visibleTopics (C-961)", () => {
  it("shows Administration's topics to instance admins only", () => {
    expect(visibleTopics("admin", { instanceAdmin: true })).toEqual([
      "overview", "internet-access", "apps", "skills", "chat-platforms", "people", "data", "platform", "ai-models", "set-in-helm", "settings",
    ]);
    expect(visibleTopics("admin", { instanceAdmin: false })).toEqual([]);
  });

  it("gives an organization member Overview, AI models and Internet access (the Manage area's sub-nav)", () => {
    expect(visibleTopics("org", { instanceAdmin: false, org: "member" })).toEqual(["overview", "ai-models", "internet-access"]);
  });

  it("gives organization owners and admins every organization topic, in the Manage area's order", () => {
    const all = ["overview", "agents", "skills", "people", "apps", "chat-platforms", "ai-models", "internet-access", "data", "settings"];
    expect(visibleTopics("org", { instanceAdmin: false, org: "owner" })).toEqual(all);
    expect(visibleTopics("org", { instanceAdmin: false, org: "admin" })).toEqual(all);
  });

  // (D-506(b)) Skills is listed to the people who own it without "Show more controls".
  const use = { floor: "use" as const, more: false };

  it("lists the organization's Skills to its owners and admins at Use, and never to its members", () => {
    for (const org of ["owner", "admin"] as const) expect(visibleTopics("org", { instanceAdmin: false, org }, use)).toEqual(["overview", "skills"]);
    expect(visibleTopics("org", { instanceAdmin: false, org: "member" }, use)).toEqual(["overview"]);
    expect(visibleTopics("org", { instanceAdmin: false, org: "member" }, { floor: "use", more: true })).not.toContain("skills");
  });

  it.each([false, true])("lists the workspace's Skills to its owner at Use (personal: %s)", (personalWorkspace) => {
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "owner", personalWorkspace }, use)).toEqual(["overview", "agents", "skills", "approvals"]);
  });

  it("keeps a member's and an approver's view at Use", () => {
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "member" }, use)).toEqual(["overview", "agents"]);
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "approver" }, use)).toEqual(["overview", "agents", "approvals"]);
  });

  it("shows an instance admin acting in an organization what its owner sees", () => {
    expect(visibleTopics("org", { instanceAdmin: true })).toEqual(visibleTopics("org", { instanceAdmin: false, org: "owner" }));
  });

  it("gives a workspace member no Approvals, People or Settings", () => {
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "member" })).toEqual([
      "overview", "agents", "skills", "apps", "internet-access", "activity",
    ]);
  });

  it("adds Approvals for an approver, and People and Settings for an owner", () => {
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "approver" })).toContain("approvals");
    expect(visibleTopics("workspace", { instanceAdmin: false, workspace: "approver" })).not.toContain("people");
    const owner = visibleTopics("workspace", { instanceAdmin: false, workspace: "owner" });
    expect(owner).toEqual(["overview", "agents", "skills", "apps", "internet-access", "approvals", "activity", "people", "settings"]);
  });

  it("shows nothing at a level the viewer has no role in", () => {
    expect(visibleTopics("org", { instanceAdmin: false })).toEqual([]);
    expect(visibleTopics("workspace", { instanceAdmin: false })).toEqual([]);
  });
});
