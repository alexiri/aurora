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
    sentence:
      "Agents reach any website but ask before changing data on sites they have no account for; only the organization's AI keys; agents run on Aurora.",
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
