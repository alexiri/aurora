import { describe, expect, it } from "vitest";

import type { ModelReadiness } from "@/lib/api/credential-grants";
import { homeTarget, personalWorkspacePath } from "@/lib/nav/landing";
import { computeGuide, type GuideKey } from "@/lib/onboarding/guide";
import { repliedPollInterval } from "@/lib/onboarding/use-guide";

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
  warnings: [],
  default: withDefault
    ? {
        endpoint_id: "litellm", model_id: "claude-sonnet", provider: "litellm", source: "endpoint_default",
        roles: {
          large: { model_id: null, source: "none", warning: null },
          medium: { model_id: null, source: "none", warning: null },
          small: { model_id: null, source: "none", warning: null },
        },
        roles_dropped: [], subagent_model: "inherit",
      }
    : null,
});
const ORG_KEY: GuideKey = { id: "k1", name: "AIGW", scope: "organization" };

const states = (g: ReturnType<typeof computeGuide>) => Object.fromEntries(g.steps.map((s) => [s.id, s.state]));

describe("getting-started guide: computed from real state (D-141)", () => {
  it("key provided by the org: two steps, the key is done and says what provides it, hello is next (C-927)", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()]), keys: [ORG_KEY], replied: false });
    expect(states(g)).toEqual({ key: "done", access: "skipped", hello: "todo" });
    expect(g.steps[0].provider).toBe("Your organization provides an AI key: AIGW");
    expect(g).toMatchObject({ total: 2, done: 1, complete: false });
  });

  it("key missing: the key step is the one to do, hello waits", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([]), keys: [], replied: false });
    expect(states(g)).toEqual({ key: "todo", access: "skipped", hello: "waiting" });
    expect(g).toMatchObject({ total: 2, done: 0 });
  });

  it("a key exists that the agent cannot use: the access step replaces the key step, naming the workspace's own key first", () => {
    const own: GuideKey = { id: "k2", name: "my-key", scope: "workspace" };
    const g = computeGuide({ isOwner: true, readiness: readiness([]), keys: [ORG_KEY, own], replied: false });
    expect(states(g)).toEqual({ key: "skipped", access: "todo", hello: "waiting" });
    expect(g.steps[1].keyToGrant).toEqual(own);
    expect(g.total).toBe(2);
  });

  it("a granted key that cannot be used keeps the key step open with the reason", () => {
    const g = computeGuide({
      isOwner: true,
      readiness: readiness([key({ usable: false, reason: "origin_unverified", scope: "workspace" })]),
      keys: [],
      replied: false,
    });
    expect(g.steps[0]).toMatchObject({ id: "key", state: "todo", problem: "origin_unverified" });
    expect(g.steps[1].state).toBe("skipped");
    expect(g.steps[2].state).toBe("waiting");
  });

  it("the key works but its endpoint has no default model: the key step stays open for a model picker; there is no third step", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()], false), keys: [ORG_KEY], replied: false });
    expect(states(g)).toEqual({ key: "todo", access: "skipped", hello: "waiting" });
    expect(g.steps[0].needsModel).toBe(true);
    expect(g.total).toBe(2);
  });

  it("done: the agent answered, whatever readiness says", () => {
    const g = computeGuide({ isOwner: true, readiness: readiness([key()]), keys: [ORG_KEY], replied: true });
    expect(states(g)).toEqual({ key: "done", access: "skipped", hello: "done" });
    expect(g).toMatchObject({ total: 2, done: 2, complete: true });
  });

  it("a click never completes anything: the same inputs give the same guide", () => {
    const input = { isOwner: true, readiness: readiness([]), keys: [] as GuideKey[], replied: false };
    expect(computeGuide(input)).toEqual(computeGuide({ ...input }));
  });

  it("member: the key is for an owner, hello is theirs; nothing is claimed done", () => {
    const g = computeGuide({ isOwner: false, keys: [], replied: false });
    expect(states(g)).toEqual({ key: "owner", access: "skipped", hello: "todo" });
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
    expect(homeTarget({ landing: null, me: undefined, isInstanceAdmin: true, placementSaved: false, welcomeSeen: false })).toBe("/admin");
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

describe("reply polling backs off and stops (FIX-067)", () => {
  it("10 s, then 30 s, then 60 s, then nothing; never once answered", () => {
    expect(repliedPollInterval(0, false)).toBe(10_000);
    expect(repliedPollInterval(5, false)).toBe(10_000);
    expect(repliedPollInterval(6, false)).toBe(30_000);
    expect(repliedPollInterval(12, false)).toBe(60_000);
    expect(repliedPollInterval(29, false)).toBe(60_000);
    expect(repliedPollInterval(30, false)).toBe(false);
    expect(repliedPollInterval(0, true)).toBe(false);
  });
});
