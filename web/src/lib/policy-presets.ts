import type { RuleInput } from "@/lib/api/policy";
import { CHANGING_METHODS } from "@/lib/internet-access";
import { BYO_POSTURES, type ByoPosture, type EffectiveGovernance, type GovForm, type OpenLane } from "@/lib/api/governance";
import type { SettingValues } from "@/lib/api/settings";

/** What the mapping reads of a setting (a `Setting` of the API has more). */
export interface SettingLike {
  key: string;
  value?: unknown;
  locked?: boolean;
}

/**
 * (P5-REFINE-18 #2, D-410, `docs/15 §Instance settings`) Policy presets: one choice that sets the instance's governance keys.
 * `PRESETS` is the mapping; `applyPreset` and `detectPreset` are its two directions. The values are the registry's own
 * (read from `crates/aurora-config/src/settings.rs`); **Open equals a fresh install's defaults** (a test pins it).
 */
export type PresetId = "open" | "balanced" | "locked_down";
export const PRESET_IDS: PresetId[] = ["open", "balanced", "locked_down"];

/** The governance keys a preset sets. `governance.egress.baseline_allowlist` and the token age are kept as they are. */
export const PRESET_KEYS = [
  "governance.egress.minimum_posture",
  "governance.egress.open_lane_minimum",
  "governance.llm.minimum_posture",
  "governance.llm.allowed_key_scopes",
  "byo.enabled",
  "byo.minimum_posture",
] as const;
export type PresetKey = (typeof PRESET_KEYS)[number];

export interface Preset {
  id: PresetId;
  label: string;
  /** The consequences, in one sentence. */
  sentence: string;
  values: Record<PresetKey, unknown>;
}

export const PRESETS: Record<PresetId, Preset> = {
  open: {
    id: "open",
    label: "Open",
    sentence: "Agents reach any website; people may add their own AI keys; owners may run agents elsewhere.",
    values: {
      "governance.egress.minimum_posture": "open",
      "governance.egress.open_lane_minimum": "enabled",
      "governance.llm.minimum_posture": "open",
      "governance.llm.allowed_key_scopes": ["organization", "workspace"],
      "byo.enabled": true,
      "byo.minimum_posture": "owners_only",
    },
  },
  balanced: {
    id: "balanced",
    label: "Balanced",
    // (D-427) The instance card changes only what the instance owns; asking before changes is each organization's Balanced.
    sentence:
      "Only the organization's AI keys, and agents run on Aurora. Each organization's Balanced also asks before an agent changes data on a site it has no account for.",
    values: {
      "governance.egress.minimum_posture": "open",
      "governance.egress.open_lane_minimum": "enabled",
      "governance.llm.minimum_posture": "open",
      "governance.llm.allowed_key_scopes": ["organization"],
      "byo.enabled": true,
      "byo.minimum_posture": "disabled",
    },
  },
  locked_down: {
    id: "locked_down",
    label: "Locked down",
    sentence: "Agents reach only the allow-list; organization keys only; no outside agents.",
    values: {
      "governance.egress.minimum_posture": "credentialed_only",
      "governance.egress.open_lane_minimum": "allowlist_only",
      "governance.llm.minimum_posture": "sanctioned_only",
      "governance.llm.allowed_key_scopes": ["organization"],
      "byo.enabled": false,
      "byo.minimum_posture": "disabled",
    },
  },
};

const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

/**
 * Detect the preset from the **effective** values of the governance section. A Helm-locked value cannot be changed here:
 * it is left out of the match, and counted in `lockedApart` when it differs from the preset's value (the card then reads
 * "Open (2 settings set in Helm)", never an error). With no exact match the answer is `custom`, `basedOn` the preset that
 * matches most keys (ties go to Open).
 */
export interface PresetState {
  /** An exact match on every key that can be changed here; null when none. */
  preset: PresetId | null;
  /** For a hand-edited set of values: the closest preset. */
  basedOn: PresetId;
  /** Helm-locked keys whose value keeps the preset from applying fully. */
  lockedApart: PresetKey[];
}

export function detectPreset(settings: SettingLike[]): PresetState {
  const by = new Map(settings.map((s) => [s.key, s]));
  let best: { id: PresetId; score: number; lockedApart: PresetKey[] } | null = null;
  let exact: { id: PresetId; lockedApart: PresetKey[] } | null = null;
  for (const id of PRESET_IDS) {
    let score = 0;
    let mismatch = 0;
    const lockedApart: PresetKey[] = [];
    for (const k of PRESET_KEYS) {
      const s = by.get(k);
      // A key the section does not carry cannot be compared: it neither matches nor breaks a match.
      if (s === undefined) continue;
      const eq = same(s.value, PRESETS[id].values[k]);
      if (s?.locked) {
        if (!eq) lockedApart.push(k);
        continue;
      }
      if (eq) score += 1;
      else mismatch += 1;
    }
    if (mismatch === 0 && exact === null) exact = { id, lockedApart };
    if (best === null || score > best.score) best = { id, score, lockedApart };
  }
  if (exact) return { preset: exact.id, basedOn: exact.id, lockedApart: exact.lockedApart };
  return { preset: null, basedOn: best?.id ?? "open", lockedApart: [] };
}

/**
 * The `values` of the PUT that applies a preset: the dashboard values already saved (`current`), with the preset's keys
 * overlaid. Helm-locked keys are never sent (the server refuses them); `skipped` names them.
 */
export function applyPreset(
  id: PresetId,
  current: SettingValues,
  settings: SettingLike[],
): { values: SettingValues; skipped: PresetKey[] } {
  const locked = new Set(settings.filter((s) => s.locked).map((s) => s.key));
  const values: SettingValues = { ...current };
  const skipped: PresetKey[] = [];
  for (const k of PRESET_KEYS) {
    if (locked.has(k)) {
      skipped.push(k);
      delete values[k];
      continue;
    }
    values[k] = PRESETS[id].values[k];
  }
  return { values, skipped };
}

// ---------------------------------------------------------------------------------------------------------------------
// (P5-REFINE-18 #2, D-410) Organization level. An organization can only tighten what the instance allows, so its three
// cards set the organization's own postures (`GovForm`), and a card looser than the instance's floor is disabled.
// ---------------------------------------------------------------------------------------------------------------------

/** The part of the organization's rules a page shows: AI models (key posture, outside agents) or Internet access. */
export type OrgPart = "models" | "internet";

export interface OrgPresetValues {
  llmPosture: "open" | "sanctioned_only";
  /** `""` = inherit the instance floor. */
  byoPosture: ByoPosture | "";
  egressPosture: "open" | "credentialed_only";
  openLane: OpenLane;
}

export const ORG_PRESETS: Record<PresetId, OrgPresetValues> = {
  open: { llmPosture: "open", byoPosture: "", egressPosture: "open", openLane: "enabled" },
  // Balanced (D-427): nobody runs agents elsewhere, and an organization rule asks before write actions (`orgAskRule`).
  balanced: { llmPosture: "open", byoPosture: "disabled", egressPosture: "open", openLane: "enabled" },
  locked_down: { llmPosture: "sanctioned_only", byoPosture: "disabled", egressPosture: "credentialed_only", openLane: "allowlist_only" },
};

const ORG_KEYS: Record<OrgPart, (keyof OrgPresetValues)[]> = {
  models: ["llmPosture", "byoPosture"],
  internet: ["egressPosture", "openLane"],
};

const LLM_RANK = ["open", "sanctioned_only"];
const EGRESS_RANK = ["open", "credentialed_only"];
const LANE_RANK = ["enabled", "allowlist_only", "disabled"];

/** What the instance already decided for the organization: the postures it may not go below. */
export interface OrgFloors {
  llmPosture: string;
  byoPosture: string;
  egressPosture: string;
  openLane: string;
}

/** The instance's floors from the organization's effective view and its governance form. */
export function orgFloors(eff: EffectiveGovernance, form: GovForm): OrgFloors {
  return {
    llmPosture: eff.llm.posture.source === "instance" ? eff.llm.posture.value : "open",
    byoPosture: form.byo.floor,
    egressPosture: eff.egress.posture.source === "instance" ? eff.egress.posture.value : "open",
    openLane: eff.egress.open_lane.source === "instance" ? eff.egress.open_lane.value : "enabled",
  };
}

const rankOf = (key: keyof OrgPresetValues): string[] =>
  key === "llmPosture" ? LLM_RANK : key === "egressPosture" ? EGRESS_RANK : key === "openLane" ? LANE_RANK : BYO_POSTURES;

/** The value a form holds for a key, with "inherit" read as the floor. */
const heldValue = (form: GovForm, floors: OrgFloors, key: keyof OrgPresetValues): string =>
  key === "byoPosture" ? form.byo.posture || floors.byoPosture : (form[key] as string);

/** (D-427) The name of the organization rule Balanced adds: asks before an agent changes data on a site it has no account for. */
export const ORG_ASK_RULE_NAME = "Ask before changing data";

/** The rule, in the existing Ask shape (an allow rule that needs a person's approval) on every website, for the changing methods. */
export function orgAskRule(): RuleInput {
  return {
    name: ORG_ASK_RULE_NAME,
    description: "Agents ask before sending anything that changes data on a website where they have no account.",
    enabled: true,
    identities: [],
    rate_limit: null,
    conditions: [],
    action: "allow",
    require_approval: true,
    targets: [{ type: "network", host: "*", methods: [...CHANGING_METHODS] }],
  };
}

/** The rules with Balanced's Ask rule added (once) or removed. */
export function withAskRule(rules: RuleInput[], on: boolean): RuleInput[] {
  const others = rules.filter((r) => r.name !== ORG_ASK_RULE_NAME);
  return on ? [...others, orgAskRule()] : others;
}

/**
 * The preset the organization's form matches on this part's keys, or `null` ("Custom"). On the Internet access part
 * Balanced needs the Ask rule in the organization's policy (`askRule`) and Open needs it absent; Locked down ignores it.
 */
export function detectOrgPreset(form: GovForm, floors: OrgFloors, part: OrgPart, askRule = false): PresetId | null {
  for (const id of PRESET_IDS) {
    const keysMatch = ORG_KEYS[part].every((k) => orgKeyMatches(id, k, heldValue(form, floors, k)));
    const askMatches = part !== "internet" || id === "locked_down" || askRule === (id === "balanced");
    // (P5-REFINE-20, org-7) A card says what people may do; the settings must say it too.
    const keysOk = part !== "models" || peopleKeysMatch(id, form);
    if (keysMatch && askMatches && keysOk) return id;
  }
  return null;
}

/** Whether the held value of one key is the card's: an "inherit" card (`""`) takes the floor, except that Open still says owners may run agents elsewhere, so it needs the held posture not to be Disabled. */
function orgKeyMatches(id: PresetId, key: keyof OrgPresetValues, held: string): boolean {
  const want = ORG_PRESETS[id][key];
  if (want !== "") return held === want;
  return key === "byoPosture" ? held !== "disabled" : true;
}

/**
 * Whether people may add their own AI key, from the endpoints' key scopes (the same value the page's switch shows):
 * true when every permitted endpoint takes workspace keys, `null` when no endpoint is permitted (nothing to say).
 */
export function peopleMayAddOwnKey(endpoints: { allowed: boolean; key_scopes: string[] }[]): boolean | null {
  const live = endpoints.filter((e) => e.allowed);
  return live.length === 0 ? null : live.every((e) => e.key_scopes.includes("workspace"));
}

/** Open says "people may add their own AI keys"; Balanced and Locked down say organization keys only. */
function peopleKeysMatch(id: PresetId, form: GovForm): boolean {
  const may = peopleMayAddOwnKey(form.endpoints);
  return may === null || may === (id === "open");
}

/** The form with a preset applied to this part's keys. */
export function applyOrgPreset(id: PresetId, form: GovForm, part: OrgPart): GovForm {
  const v = ORG_PRESETS[id];
  const next: GovForm = { ...form, byo: { ...form.byo } };
  for (const k of ORG_KEYS[part]) {
    if (k === "byoPosture") next.byo.posture = v.byoPosture;
    else if (k === "llmPosture") next.llmPosture = v.llmPosture;
    else if (k === "egressPosture") next.egressPosture = v.egressPosture;
    else next.openLane = v.openLane;
  }
  if (part === "models") {
    // Who may add their own AI key follows the card (an endpoint whose scopes the instance set stays as it is).
    next.endpoints = form.endpoints.map((e) => {
      if (!e.allowed || e.locked.key_scopes) return e;
      const rest = e.key_scopes.filter((s) => s !== "workspace");
      return { ...e, key_scopes: id === "open" ? [...rest, "workspace"] : rest };
    });
  }
  return next;
}

/**
 * Why a card cannot be chosen: its value is looser than what the instance set, or an endpoint's key scopes the instance
 * set say the opposite of the card; `null` when it can.
 */
export function orgPresetForbidden(id: PresetId, floors: OrgFloors, part: OrgPart, form?: GovForm): string | null {
  const why = "Set by the instance admin";
  for (const k of ORG_KEYS[part]) {
    const want = ORG_PRESETS[id][k];
    if (want === "") {
      // Open leaves agents elsewhere to the instance floor, but says owners may run them: not under a floor of Disabled.
      if (k === "byoPosture" && floors.byoPosture === "disabled") return why;
      continue;
    }
    const rank = rankOf(k);
    if (rank.indexOf(want) < rank.indexOf(floors[k])) return why;
  }
  if (part === "models" && form?.endpoints.some((e) => e.allowed && e.locked.key_scopes && e.key_scopes.includes("workspace") !== (id === "open"))) return why;
  return null;
}
