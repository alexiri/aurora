import { describe, expect, it } from "vitest";

import { firstNameOf, nameInSentence, personName, workspaceDisplayName } from "@/lib/workspace-name";

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
  it("never shows an e-mail address as a person's name (batch 6)", () => {
    expect(workspaceDisplayName({ name: "dave@example.test", personal_of: { id: "u3", name: "dave@example.test" } }, me)).toBe("Dave's workspace");
    expect(workspaceDisplayName({ name: "x", personal_of: { id: "u3", name: "  " } }, me)).toBe("Workspace of a person without a name");
    expect(workspaceDisplayName({ name: "x", personal_of: { id: "u3", name: "@example.test" } }, me)).toBe("Workspace of a person without a name");
    expect(firstNameOf("Bob Builder")).toBe("Bob");
  });
  it("personName: the display name, else the local part of the address, else 'A person without a name'; never an e-mail (batch 7 c)", () => {
    expect(personName("Bob Builder")).toBe("Bob Builder");
    expect(personName("dave@example.test")).toBe("Dave");
    expect(personName("  ")).toBe("A person without a name");
    expect(personName(null)).toBe("A person without a name");
    expect(personName("@example.test")).toBe("A person without a name");
  });
  it("reads 'Your workspace' in lower case after a verb", () => {
    expect(nameInSentence("Your workspace")).toBe("your workspace");
    expect(nameInSentence("Support team")).toBe("Support team");
    expect(nameInSentence("Bob's workspace")).toBe("Bob's workspace");
  });
  it("keeps the stored name for shared workspaces, and without a viewer", () => {
    expect(workspaceDisplayName({ name: "Support team" }, me)).toBe("Support team");
    expect(workspaceDisplayName({ name: "Support team", personal_of: null }, undefined)).toBe("Support team");
    expect(workspaceDisplayName({ name: "Carol Danvers", personal_of: { id: "u2", name: "Carol Danvers" } }, undefined)).toBe("Carol's workspace");
  });
});
