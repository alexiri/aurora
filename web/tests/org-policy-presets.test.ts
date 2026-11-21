import { describe, expect, it } from "vitest";

import type { EffectiveGovernance, GovForm } from "@/lib/api/governance";
import { applyOrgPreset, detectOrgPreset, orgAskRule, orgFloors, orgPresetForbidden, ORG_ASK_RULE_NAME, whereKeysMayBeAdded, withAskRule } from "@/lib/policy-presets";

const sourced = (value: string, source = "org") => ({ value, source });
const eff = (over: { llm?: [string, string]; egress?: [string, string]; lane?: [string, string] } = {}) =>
  ({
    llm: { posture: sourced(...(over.llm ?? ["open", "org"])), endpoints: [] },
    egress: { posture: sourced(...(over.egress ?? ["open", "org"])), open_lane: sourced(...(over.lane ?? ["enabled", "org"])), baseline_allowlist: [], org_allowlist: [] },
    byo: { posture: sourced("enabled", "org") },
  }) as unknown as EffectiveGovernance;
const form = (over: Partial<GovForm> = {}, byo: Partial<GovForm["byo"]> = {}): GovForm =>
  ({
    llmPosture: "open",
    egressPosture: "open",
    openLane: "enabled",
    allowlist: [],
    endpoints: [],
    byo: { posture: "", floor: "enabled", ...byo },
    ...over,
  }) as unknown as GovForm;

describe("organization policy presets (P5-REFINE-18 #2)", () => {
  it("applies each preset to its part's keys only, and detects it back", () => {
    for (const id of ["open", "balanced", "locked_down"] as const) {
      const f = form();
      const floors = orgFloors(eff(), f);
      for (const part of ["models", "internet"] as const) {
        const next = applyOrgPreset(id, f, part);
        // On Internet access Balanced is the Ask rule being on (D-427); the other two do not need it.
        expect(detectOrgPreset(next, floors, part, id === "balanced")).toBe(id);
      }
    }
    const locked = applyOrgPreset("locked_down", form(), "internet");
    expect(locked.egressPosture).toBe("credentialed_only");
    expect(locked.openLane).toBe("allowlist_only");
    // The other part is untouched.
    expect(locked.llmPosture).toBe("open");
    expect(locked.byo.posture).toBe("");
    const models = applyOrgPreset("locked_down", form(), "models");
    expect(models.llmPosture).toBe("sanctioned_only");
    expect(models.byo.posture).toBe("disabled");
    expect(models.egressPosture).toBe("open");
  });

  it("a hand-edited set of values is Custom", () => {
    const f = form({ egressPosture: "credentialed_only", openLane: "enabled" });
    expect(detectOrgPreset(f, orgFloors(eff(), f), "internet")).toBeNull();
  });

  it("reads 'inherit' as the instance floor, and Open never claims agents elsewhere under a floor of Disabled (org-7)", () => {
    const f = form({}, { posture: "", floor: "disabled" });
    const floors = orgFloors(eff(), f);
    // Open's "inherit" would be Disabled here, which contradicts "owners may run agents elsewhere": Balanced is the match.
    expect(detectOrgPreset(f, floors, "models")).toBe("balanced");
    expect(orgPresetForbidden("open", floors, "models")).toBe("Set by the instance admin");
    const free = form({}, { posture: "", floor: "owners_only" });
    expect(detectOrgPreset(free, orgFloors(eff(), free), "models")).toBe("open");
  });

  const endpoint = (scopes: string[], over: Record<string, unknown> = {}) => ({ id: "anthropic", allowed: true, key_scopes: scopes, locked: { permitted: false, key_scopes: false, allowed_models: false }, ...over });

  it("never claims Open while people may not add their own AI key, and the card writes what it says (org-7)", () => {
    const orgKeysOnly = form({ endpoints: [endpoint(["organization"])] as never });
    const floors = orgFloors(eff(), orgKeysOnly);
    // The summary reads "People may not add their own": Open's sentence says they may.
    // Not Open; and not Balanced either, since agents elsewhere are still allowed (the floor): Custom.
    expect(detectOrgPreset(orgKeysOnly, floors, "models")).toBeNull();
    const open = applyOrgPreset("open", orgKeysOnly, "models");
    expect(open.endpoints[0]!.key_scopes).toEqual(["organization", "workspace"]);
    expect(detectOrgPreset(open, floors, "models")).toBe("open");
    const back = applyOrgPreset("balanced", open, "models");
    expect(back.endpoints[0]!.key_scopes).toEqual(["organization"]);
    // An endpoint that is not permitted, or whose scopes the instance set, is left alone.
    const fixed = form({ endpoints: [endpoint(["organization"], { locked: { permitted: false, key_scopes: true, allowed_models: false } }), endpoint([], { allowed: false })] as never });
    expect(applyOrgPreset("open", fixed, "models").endpoints.map((e) => e.key_scopes)).toEqual([["organization"], []]);
    // (batch 6) Key scopes the instance set (the starter key's endpoint takes organization keys only) are the instance's decision:
    // they neither forbid Open ("Set by the instance admin" on an Open instance) nor make the card Custom.
    expect(orgPresetForbidden("open", floors, "models")).toBeNull();
    expect(orgPresetForbidden("balanced", floors, "models")).toBeNull();
    expect(detectOrgPreset(fixed, floors, "models")).toBe("open");
  });

  it("says where people may add their own key: every live endpoint, some by name, none, or nothing to say (org-7, pass 3)", () => {
    const e = (id: string, scopes: string[], allowed = true) => ({ id, display_name: id.toUpperCase(), allowed, key_scopes: scopes });
    expect(whereKeysMayBeAdded([])).toBeNull();
    expect(whereKeysMayBeAdded([e("a", [], false)])).toBeNull();
    expect(whereKeysMayBeAdded([e("a", ["organization", "workspace"]), e("b", ["workspace"])])).toEqual({ kind: "all", names: ["A", "B"] });
    expect(whereKeysMayBeAdded([e("a", ["organization"]), e("b", ["workspace"]), e("c", ["workspace"], false)])).toEqual({ kind: "some", names: ["B"] });
    expect(whereKeysMayBeAdded([e("a", ["organization"])])).toEqual({ kind: "none", names: [] });
  });

  it("disables a card that is looser than what the instance admin set, with the reason", () => {
    const f = form({}, { floor: "disabled" });
    const floors = orgFloors(eff({ egress: ["credentialed_only", "instance"], lane: ["allowlist_only", "instance"], llm: ["sanctioned_only", "instance"] }), f);
    expect(orgPresetForbidden("open", floors, "internet")).toBe("Set by the instance admin");
    expect(orgPresetForbidden("balanced", floors, "internet")).toBe("Set by the instance admin");
    expect(orgPresetForbidden("locked_down", floors, "internet")).toBeNull();
    expect(orgPresetForbidden("open", floors, "models")).toBe("Set by the instance admin");
    // Without an instance decision nothing is forbidden.
    const free = orgFloors(eff(), form());
    for (const id of ["open", "balanced", "locked_down"] as const) expect(orgPresetForbidden(id, free, "internet")).toBeNull();
  });
});

describe("organization Balanced asks before changes (D-427)", () => {
  it("Balanced on Internet access is Open's governance plus the Ask rule; without the rule it reads as Open", () => {
    const f = applyOrgPreset("balanced", form(), "internet");
    const floors = orgFloors(eff(), f);
    expect(detectOrgPreset(f, floors, "internet", true)).toBe("balanced");
    expect(detectOrgPreset(f, floors, "internet", false)).toBe("open");
    // Locked down ignores the rule either way.
    const locked = applyOrgPreset("locked_down", form(), "internet");
    expect(detectOrgPreset(locked, floors, "internet", true)).toBe("locked_down");
    expect(detectOrgPreset(locked, floors, "internet", false)).toBe("locked_down");
    // The AI models part has no rule.
    expect(detectOrgPreset(applyOrgPreset("balanced", form(), "models"), floors, "models")).toBe("balanced");
  });

  it("the rule is the existing Ask shape: an allow rule that needs approval, on every website, for the changing methods", () => {
    const rule = orgAskRule();
    expect(rule).toMatchObject({ name: ORG_ASK_RULE_NAME, action: "allow", require_approval: true, enabled: true });
    expect(rule.targets).toEqual([{ type: "network", host: "*", methods: ["POST", "PUT", "PATCH", "DELETE"] }]);
  });

  it("adds the rule once and removes it again, leaving the other rules alone", () => {
    const other = { ...orgAskRule(), name: "Block example.org", action: "block" as const, require_approval: false };
    const on = withAskRule([other], true);
    expect(on.map((r) => r.name)).toEqual(["Block example.org", ORG_ASK_RULE_NAME]);
    expect(withAskRule(on, true).filter((r) => r.name === ORG_ASK_RULE_NAME)).toHaveLength(1);
    expect(withAskRule(on, false).map((r) => r.name)).toEqual(["Block example.org"]);
  });
});
