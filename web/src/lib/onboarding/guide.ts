import type { ModelReadiness } from "@/lib/api/credential-grants";

/**
 * The getting-started guide (D-141, `14 §Onboarding`). Every step is **computed** from real state (model
 * readiness, the keys that exist, a conversation with a reply). Nothing here is stored and nothing counts clicks.
 */

export type GuideStepId = "key" | "access" | "model" | "hello";

/**
 * - `done`: satisfied (collapsed, with what provides it).
 * - `todo`: the person can do it now.
 * - `waiting`: an earlier step comes first.
 * - `owner`: a workspace owner has to do it; the person is a member.
 * - `skipped`: not needed (a model is only a step when none resolves).
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
  /** Active LLM keys the person can see in the workspace or its organization (granted or not). */
  keys: GuideKey[];
  /** A conversation of the agent has an answer from it. */
  replied: boolean;
}

export interface GuideStep {
  id: GuideStepId;
  state: GuideStepState;
  /** `done` only: what provides it, in words ("Your organization provides an LLM key: AIGW"). */
  provider?: string;
  /** A granted key that cannot be used: the first reason code (`not_active`, `origin_unverified`, …). */
  problem?: string;
  /** `todo` for access: the key to give the agent. */
  keyToGrant?: GuideKey;
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
      { id: "access", state: "done" },
      { id: "model", state: "skipped" },
      { id: "hello", state: "done" },
    ];
  } else if (!isOwner) {
    steps = [
      { id: "key", state: "owner" },
      { id: "access", state: "owner" },
      { id: "model", state: "skipped" },
      { id: "hello", state: "todo" },
    ];
  } else if (!readiness) {
    // Loading: nothing is claimed yet.
    steps = [
      { id: "key", state: "waiting" },
      { id: "access", state: "waiting" },
      { id: "model", state: "skipped" },
      { id: "hello", state: "waiting" },
    ];
  } else {
    const granted = readiness.keys;
    const usable = granted.filter((k) => k.usable);
    const hasKey = granted.length > 0 || keys.length > 0;
    const unusable = granted.find((k) => !k.usable);

    const key: GuideStep =
      usable.length > 0
        ? { id: "key", state: "done", provider: providerText(usable[0].scope, usable[0].name) }
        : granted.length > 0
          ? // A key is granted but cannot be used: the person fixes the key, not the grant.
            { id: "key", state: "todo", problem: unusable?.reason ?? "not_active" }
          : hasKey
            ? { id: "key", state: "done", provider: providerText(keys[0].scope, keys[0].name) }
            : { id: "key", state: "todo" };

    const access: GuideStep =
      granted.length > 0
        ? { id: "access", state: "done" }
        : hasKey
          ? { id: "access", state: "todo", keyToGrant: keys.find((k) => k.scope === "workspace") ?? keys[0] }
          : { id: "access", state: "waiting" };

    const needsModel = usable.length > 0 && !readiness.default;
    const model: GuideStep = needsModel
      ? { id: "model", state: "todo" }
      : { id: "model", state: "skipped" };

    const ready = usable.length > 0 && !needsModel;
    steps = [key, access, model, { id: "hello", state: ready ? "todo" : "waiting" }];
  }

  const counted = steps.filter((s) => s.state !== "skipped");
  const done = counted.filter((s) => s.state === "done").length;
  return { steps, total: counted.length, done, complete: counted.length > 0 && done === counted.length };
}

function providerText(scope: string, name: string): string {
  return scope === "organization"
    ? `Your organization provides an LLM key: ${name}`
    : `This workspace has an LLM key: ${name}`;
}

/** The first message the guide suggests to say hello. */
export const HELLO_MESSAGE = "Hello! What can you help me with?";
