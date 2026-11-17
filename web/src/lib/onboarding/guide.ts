import type { ModelReadiness } from "@/lib/api/credential-grants";

/**
 * The getting-started guide (D-141, P5-UX-7, C-927, `14 §9`). Two steps, **computed** from real state (model readiness,
 * the keys that exist, a conversation with a reply): *AI key*, then *Say hello*. A third, "Give <Agent> access to <key>",
 * exists only while a key exists that the agent cannot use. A model is not a step: the key's own step shows the picker
 * when the key's endpoint has no default model. Nothing here is stored and nothing counts clicks.
 */

export type GuideStepId = "key" | "access" | "hello";

/**
 * - `done`: satisfied (collapsed, with what provides it).
 * - `todo`: the person can do it now.
 * - `waiting`: an earlier step comes first.
 * - `owner`: a workspace owner has to do it; the person is a member.
 * - `skipped`: not a step now (a key the agent cannot use is the only reason for "access").
 */
export type GuideStepState = "done" | "todo" | "waiting" | "owner" | "skipped";

export interface GuideKey {
  id: string;
  name: string;
  scope: "workspace" | "organization";
}

export interface GuideInput {
  /** Owner of the workspace (an org owner or admin counts). Readiness is readable by owners only (C-500). */
  isOwner: boolean;
  /** Per-key readiness of the agent; `undefined` for a member or while loading. */
  readiness?: ModelReadiness;
  /** Active AI keys the person can see in the workspace or its organization (granted or not). */
  keys: GuideKey[];
  /** A conversation of the agent has an answer from it. */
  replied: boolean;
}

export interface GuideStep {
  id: GuideStepId;
  state: GuideStepState;
  /** `done` only: what provides it, in words ("Your organization provides an AI key: AIGW"). */
  provider?: string;
  /** A granted key that cannot be used: the first reason code (`not_active`, `origin_unverified`, …). */
  problem?: string;
  /** `todo` for access: the key to give the agent. */
  keyToGrant?: GuideKey;
  /** `todo` for the key: the agent can use a key, but no model resolves, so the step asks for one. */
  needsModel?: boolean;
}

export interface Guide {
  steps: GuideStep[];
  /** Steps that count (everything but `skipped`). */
  total: number;
  done: number;
  complete: boolean;
}

export function computeGuide(input: GuideInput): Guide {
  const { isOwner, readiness, keys, replied } = input;
  let steps: GuideStep[];

  if (replied) {
    // The agent answered: whatever was missing is not any more. Show what provides the answer when known.
    const usable = readiness?.keys.find((k) => k.usable);
    steps = [
      { id: "key", state: "done", provider: usable ? providerText(usable.scope, usable.name) : undefined },
      { id: "access", state: "skipped" },
      { id: "hello", state: "done" },
    ];
  } else if (!isOwner) {
    steps = [
      { id: "key", state: "owner" },
      { id: "access", state: "skipped" },
      { id: "hello", state: "todo" },
    ];
  } else if (!readiness) {
    // Loading: nothing is claimed yet.
    steps = [
      { id: "key", state: "waiting" },
      { id: "access", state: "skipped" },
      { id: "hello", state: "waiting" },
    ];
  } else {
    const granted = readiness.keys;
    const usable = granted.filter((k) => k.usable);
    const unusable = granted.find((k) => !k.usable);

    if (usable.length > 0) {
      const provider = providerText(usable[0].scope, usable[0].name);
      // The key works; without a default model the step stays open to pick one.
      steps = readiness.default
        ? [{ id: "key", state: "done", provider }, { id: "access", state: "skipped" }, { id: "hello", state: "todo" }]
        : [{ id: "key", state: "todo", needsModel: true, provider }, { id: "access", state: "skipped" }, { id: "hello", state: "waiting" }];
    } else if (granted.length > 0) {
      // A key is granted but cannot be used: the person fixes the key, not the grant.
      steps = [
        { id: "key", state: "todo", problem: unusable?.reason ?? "not_active" },
        { id: "access", state: "skipped" },
        { id: "hello", state: "waiting" },
      ];
    } else if (keys.length > 0) {
      // A key exists that the agent cannot use: the one step is to give it access (the key step is not shown).
      steps = [
        { id: "key", state: "skipped" },
        { id: "access", state: "todo", keyToGrant: keys.find((k) => k.scope === "workspace") ?? keys[0] },
        { id: "hello", state: "waiting" },
      ];
    } else {
      steps = [{ id: "key", state: "todo" }, { id: "access", state: "skipped" }, { id: "hello", state: "waiting" }];
    }
  }

  const counted = steps.filter((s) => s.state !== "skipped");
  const done = counted.filter((s) => s.state === "done").length;
  return { steps, total: counted.length, done, complete: counted.length > 0 && done === counted.length };
}

function providerText(scope: string, name: string): string {
  return scope === "organization" ? `Your organization provides an AI key: ${name}` : `This workspace has an AI key: ${name}`;
}

/** The first message the guide suggests to say hello. */
export const HELLO_MESSAGE = "Hello! What can you help me with?";
