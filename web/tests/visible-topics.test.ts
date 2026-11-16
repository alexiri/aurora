import { describe, expect, it } from "vitest";

import { visibleTopics } from "@/lib/api/roles";

describe("visibleTopics (C-961)", () => {
  it("shows Administration's topics to instance admins only", () => {
    expect(visibleTopics("admin", { instanceAdmin: true })).toEqual([
      "overview", "agents", "apps", "ai-models", "internet-access", "people", "chat-platforms", "data", "platform", "settings",
    ]);
    expect(visibleTopics("admin", { instanceAdmin: false })).toEqual([]);
  });

  it("gives an organization member Workspaces, AI models and Internet access", () => {
    expect(visibleTopics("org", { instanceAdmin: false, org: "member" })).toEqual(["workspaces", "ai-models", "internet-access"]);
  });

  it("gives organization owners and admins every organization topic", () => {
    const all = ["workspaces", "agents", "apps", "ai-models", "internet-access", "people", "chat-platforms", "data", "settings"];
    expect(visibleTopics("org", { instanceAdmin: false, org: "owner" })).toEqual(all);
    expect(visibleTopics("org", { instanceAdmin: false, org: "admin" })).toEqual(all);
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
