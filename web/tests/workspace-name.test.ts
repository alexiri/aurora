import { describe, expect, it } from "vitest";

import { workspaceDisplayName } from "@/lib/workspace-name";

describe("workspaceDisplayName (D-389)", () => {
  const me = { id: "u1" };
  it("reads the viewer's own personal workspace as 'Your workspace'", () => {
    expect(workspaceDisplayName({ name: "Bob Builder", personal_of: { id: "u1", name: "Bob Builder" } }, me)).toBe("Your workspace");
    expect(workspaceDisplayName({ name: "Bob Builder", personal: true }, me)).toBe("Your workspace");
  });
  it("reads someone else's as '<First name>'s workspace'", () => {
    expect(workspaceDisplayName({ name: "Carol Danvers", personal_of: { id: "u2", name: "Carol Danvers" } }, me)).toBe("Carol's workspace");
  });
  it("uses the whole display name when it has one word", () => {
    expect(workspaceDisplayName({ name: "Daneel", personal_of: { id: "u2", name: "Daneel" } }, me)).toBe("Daneel's workspace");
  });
  it("keeps the stored name for shared workspaces, and without a viewer", () => {
    expect(workspaceDisplayName({ name: "Support team" }, me)).toBe("Support team");
    expect(workspaceDisplayName({ name: "Support team", personal_of: null }, undefined)).toBe("Support team");
    expect(workspaceDisplayName({ name: "Carol Danvers", personal_of: { id: "u2", name: "Carol Danvers" } }, undefined)).toBe("Carol's workspace");
  });
});
