import { describe, expect, it } from "vitest";

import type { ModelReadiness } from "@/lib/api/credential-grants";
import { homeTarget, personalWorkspacePath } from "@/lib/nav/landing";
import { computeGuide, type GuideKey } from "@/lib/onboarding/guide";

const key = (over: Partial<ModelReadiness["keys"][number]> = {}): ModelReadiness["keys"][number] => ({
  secret_id: "k1",
  name: "AIGW",
  scope: "organization",
  provider: "litellm",
  endpoint_id: "litellm",
  usable: true,
  reason: null,
  ...over,
});
const readiness = (keys: ModelReadiness["keys"], withDefault = true): ModelReadiness => ({
  harness: "claude-code",
  keys,
  override_dropped: false,
  default: withDefault ? { endpoint_id: "litellm", model_id: "claude-sonnet", provider: "litellm", source: "endpoint_default" } : null,
});
const ORG_KEY: GuideKey = { id: "k1", name: "AIGW", scope: "organization" };

const states = (g: ReturnType<typeof computeGuide>) => Object.fromEntries(g.steps.map((s) => [s.id, s.state]));

describe("getting-started guide: computed from real state (D-141)", () => {
  it("key provided by the org: key and access are done and say what provides them; a model is not needed", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()]), keys: [ORG_KEY], replied: false });
    expect(states(g)).toEqual({ key: "done", access: "done", model: "skipped", hello: "todo" });
    expect(g.steps[0].provider).toBe("Your organization provides an LLM key: AIGW");
    expect(g).toMatchObject({ total: 3, done: 2, complete: false });
  });

  it("key missing: the key step is the one to do, the rest wait", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([]), keys: [], replied: false });
    expect(states(g)).toEqual({ key: "todo", access: "waiting", model: "skipped", hello: "waiting" });
    expect(g.done).toBe(0);
  });

  it("a key exists but is not granted: access is the step, preselecting the workspace's own key first", () => {
    const own: GuideKey = { id: "k2", name: "my-key", scope: "workspace" };
    const g = computeGuide({ isOwner: true, readiness: readiness([]), keys: [ORG_KEY, own], replied: false });
    expect(states(g)).toEqual({ key: "done", access: "todo", model: "skipped", hello: "waiting" });
    expect(g.steps[1].keyToGrant).toEqual(own);
  });

  it("a granted key that cannot be used keeps the key step open with the reason", () => {
    const g = computeGuide({
      isOwner: true,
      readiness: readiness([key({ usable: false, reason: "origin_unverified", scope: "workspace" })]),
      keys: [],
      replied: false,
    });
    expect(g.steps[0]).toMatchObject({ id: "key", state: "todo", problem: "origin_unverified" });
    expect(g.steps[1].state).toBe("done");
    expect(g.steps[3].state).toBe("waiting");
  });

  it("no model resolves: the model step appears and blocks hello", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()], false), keys: [ORG_KEY], replied: false });
    expect(states(g)).toEqual({ key: "done", access: "done", model: "todo", hello: "waiting" });
    expect(g.total).toBe(4);
  });

  it("done: the agent answered, whatever readiness says", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()]), keys: [ORG_KEY], replied: true });
    expect(states(g)).toEqual({ key: "done", access: "done", model: "skipped", hello: "done" });
    expect(g).toMatchObject({ total: 3, done: 3, complete: true });
  });

  it("a click never completes anything: the same inputs give the same guide", () => {
    const input = { isOwner: true, readiness: readiness([]), keys: [] as GuideKey[], replied: false };
    expect(computeGuide(input)).toEqual(computeGuide({ ...input }));
  });

  it("member: key and access are for an owner, hello is theirs; nothing is claimed done", () => {
    const g = computeGuide({ isOwner: false, keys: [], replied: false });
    expect(states(g)).toEqual({ key: "owner", access: "owner", model: "skipped", hello: "todo" });
    expect(g.done).toBe(0);
  });

  it("member whose agent answered: the whole guide is done", () => {
    const g = computeGuide({ isOwner: false, keys: [], replied: true });
    expect(g.complete).toBe(true);
  });

  it("while readiness loads an owner's guide claims nothing", () => {
    const g = computeGuide({ isOwner: true, keys: [], replied: false });
    expect(g.done).toBe(0);
    expect(g.steps.every((s) => s.state === "waiting" || s.state === "skipped")).toBe(true);
  });
});

describe("where `/` sends people (D-141)", () => {
  const me = {
    memberships: [
      {
        org: { slug: "users" },
        status: "active",
        workspaces: [
          { slug: "main", personal: false },
          { slug: "ada", personal: true },
        ],
      },
    ],
  };

  it("finds the personal workspace", () => {
    expect(personalWorkspacePath(me)).toBe("/o/users/w/ada");
    expect(personalWorkspacePath(undefined)).toBeNull();
  });

  it("an instance admin without a saved placement gets the setup wizard, even without an organization", () => {
    expect(homeTarget({ landing: null, me: undefined, isInstanceAdmin: true, placementSaved: false, welcomeSeen: false })).toBe("/admin/setup");
    expect(homeTarget({ landing: "/o/a/w/b", me, isInstanceAdmin: false, placementSaved: false, welcomeSeen: true })).toBe("/o/a/w/b");
  });

  it("a person who has not seen Welcome lands on it in their personal workspace, else in the landing workspace", () => {
    expect(homeTarget({ landing: "/o/users/w/main", me, isInstanceAdmin: false, placementSaved: true, welcomeSeen: false })).toBe("/o/users/w/ada/welcome");
    expect(homeTarget({ landing: "/o/users/w/main", me: { memberships: [] }, isInstanceAdmin: false, placementSaved: true, welcomeSeen: false })).toBe(
      "/o/users/w/main/welcome",
    );
  });

  it("everyone else lands on the workspace; an unknown Welcome state never redirects", () => {
    expect(homeTarget({ landing: "/o/users/w/main", me, isInstanceAdmin: false, placementSaved: true, welcomeSeen: true })).toBe("/o/users/w/main");
    expect(homeTarget({ landing: "/o/users/w/main", me, isInstanceAdmin: true, placementSaved: undefined, welcomeSeen: undefined })).toBe("/o/users/w/main");
  });
});
