import { describe, expect, it } from "vitest";

// (FIX-235, D-479) One row of the brief's table per case, plus the text-rendering rule.
import { visibilityLines } from "@/lib/chat/audience";

const audience = (people: number, owners: [string, string][], ownersTotal = owners.length) => ({
  people,
  owners: owners.map(([id, display_name]) => ({ id, display_name })),
  owners_total: ownersTotal,
});

const base = {
  kind: "direct",
  ownDirect: true,
  ownerView: null,
  readerId: "me",
  myRole: "member",
};

describe("visibilityLines", () => {
  it("N = 1, any conversation: only you", () => {
    expect(visibilityLines({ ...base, audience: audience(1, [["me", "Me"]]) })).toEqual([
      "Only you can use this agent and read its conversations.",
    ]);
    expect(
      visibilityLines({ ...base, kind: "group", ownDirect: false, audience: audience(1, [["me", "Me"]]) }),
    ).toEqual(["Only you can use this agent and read its conversations."]);
  });

  it("own direct chat, the reader the only owner: only you can read this chat", () => {
    expect(visibilityLines({ ...base, myRole: "owner", audience: audience(3, [["me", "Me"]]) })).toEqual([
      "All 3 people in this workspace can use this agent. Only you can read this chat.",
    ]);
  });

  it("own direct chat, one other owner: singular", () => {
    expect(visibilityLines({ ...base, audience: audience(3, [["a", "Ann"]], 1) })).toEqual([
      "All 3 people in this workspace can use this agent. This chat is visible to you and the workspace's owner, Ann.",
    ]);
  });

  it("own direct chat, two or more other owners: the list", () => {
    expect(visibilityLines({ ...base, audience: audience(4, [["a", "Ann"], ["b", "Bo"]], 2) })).toEqual([
      "All 4 people in this workspace can use this agent. This chat is visible to you and the workspace's owners: Ann, Bo.",
    ]);
  });

  it("four or more owners: three names, then and K others", () => {
    expect(
      visibilityLines({
        ...base,
        audience: audience(8, [["a", "Ann"], ["b", "Bo"], ["c", "Cy"], ["d", "Dee"], ["e", "Eli"]], 5),
      }),
    ).toEqual([
      "All 8 people in this workspace can use this agent. This chat is visible to you and the workspace's owners: Ann, Bo, Cy and 2 others.",
    ]);
  });

  it("the reader an owner with other owners: the other owners, never the reader", () => {
    expect(
      visibilityLines({
        ...base,
        myRole: "owner",
        audience: audience(5, [["me", "Me"], ["a", "Ann"], ["b", "Bo"]], 3),
      }),
    ).toEqual(["All 5 people in this workspace can use this agent. This chat is visible to you and the other owners: Ann, Bo."]);
    // One other owner: singular.
    expect(
      visibilityLines({
        ...base,
        myRole: "owner",
        audience: audience(4, [["me", "Me"], ["a", "Ann"]], 2),
      }),
    ).toEqual(["All 4 people in this workspace can use this agent. This chat is visible to you and the other owner: Ann."]);
  });

  it("not direct: everyone with access reads it", () => {
    expect(
      visibilityLines({ ...base, kind: "group", ownDirect: false, audience: audience(3, [["a", "Ann"]], 1) }),
    ).toEqual(["All 3 people in this workspace can use this agent and read this conversation."]);
  });

  it("an owner reading another person's direct chat: the read-only line", () => {
    expect(
      visibilityLines({ ...base, ownDirect: false, ownerView: "Ann", audience: audience(3, [["me", "Me"], ["a", "Ann"]], 2) }),
    ).toEqual(["Only Ann and the workspace's owners can see this conversation."]);
  });

  it("names render as text, never HTML", () => {
    const [line] = visibilityLines({
      ...base,
      audience: audience(2, [["a", "Ann <b> & Friends"]], 1),
    });
    expect(line).toContain("Ann <b> & Friends");
    expect(line).not.toContain("&amp;");
  });
});
