import type { WhoCanValue } from "@/lib/who-can";
import type { LadderAbove } from "@/components/ui/scope-ladder";
import type { DraftState, RuleInput } from "@/lib/api/policy";
import type { EffectiveGovernance } from "@/lib/api/governance";

/** (C-989) The methods that change something on a website; "Ask before changes" names them. */
export const CHANGING_METHODS = ["POST", "PUT", "PATCH", "DELETE"] as const;

export type CommonChange = "ask_before_changes" | "block";

/**
 * The host a person typed ("https://Status.Example.org/path" → "status.example.org"); `null` for nothing usable. A leading
 * `*.` is kept (the rule's own host patterns accept it).
 */
export function hostOf(input: string): string | null {
  let s = input.trim().toLowerCase();
  if (!s) return null;
  s = s.replace(/^[a-z][a-z0-9+.-]*:\/\//, "").replace(/[/?#].*$/, "").replace(/:\d+$/, "");
  return /^(\*\.)?[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/.test(s.replace(/^\*\./, "")) ? s : null;
}

/**
 * (C-989) The ordinary rule a Common change adds to the draft. **Ask before changes on a website** is the behaviour of
 * the access choice, for a site where the agent has no account: methods `POST|PUT|PATCH|DELETE` on the host, allowed
 * only with a person's approval. **Block a website** blocks every request to it.
 */
export function commonChangeRule(kind: CommonChange, host: string): RuleInput {
  const base = { description: null, enabled: true, identities: [], rate_limit: null, conditions: [] };
  return kind === "ask_before_changes"
    ? {
        ...base,
        name: `Ask before changes on ${host}`,
        action: "allow",
        require_approval: true,
        targets: [{ type: "network", host, methods: [...CHANGING_METHODS] }],
      }
    : { ...base, name: `Block ${host}`, action: "block", require_approval: false, targets: [{ type: "network", host }] };
}

const strip = (r: RuleInput) => JSON.stringify({ ...r, logical_id: undefined });

/** True when a rule of the draft is new or edited since the published generation ("Not applied yet"). */
export function ruleIsPending(rule: RuleInput, published: DraftState): boolean {
  if (!rule.logical_id) return true;
  const was = published.rules.find((r) => r.logical_id === rule.logical_id);
  return !was || strip(was) !== strip(rule);
}

/**
 * How many changes the draft holds that are not applied: rules added, edited or removed, and each of the defaults that
 * differ (the default rule, requests without an account, browser connections, the allow-list).
 */
export function pendingChanges(published: DraftState, draft: DraftState): number {
  let n = 0;
  for (const r of draft.rules) if (ruleIsPending(r, published)) n += 1;
  for (const r of published.rules) if (!draft.rules.some((d) => d.logical_id === r.logical_id)) n += 1;
  // Rules that only moved are a change too: the order decides which rule matches first.
  const kept = (rules: RuleInput[], other: RuleInput[]) => rules.filter((r) => r.logical_id && other.some((o) => o.logical_id === r.logical_id)).map((r) => r.logical_id);
  if (JSON.stringify(kept(draft.rules, published.rules)) !== JSON.stringify(kept(published.rules, draft.rules))) n += 1;
  if (published.default_action !== draft.default_action) n += 1;
  if (published.uncredentialed_default !== draft.uncredentialed_default) n += 1;
  if (published.open_lane_posture !== draft.open_lane_posture) n += 1;
  const list = (d: DraftState) => JSON.stringify(d.uncredentialed_allowlist.filter((e) => e.host.trim()).map((e) => [e.host.trim(), e.path?.trim() || null]));
  if (list(published) !== list(draft)) n += 1;
  return n;
}

const POSTURE_WORDS: Record<string, string> = { open: "Any website", credentialed_only: "Only listed websites" };
const LANE_WORDS: Record<string, string> = {
  enabled: "Allowed",
  allowlist_only: "Only for listed websites",
  disabled: "Turned off",
};

function listWords(entries: { host: string; path?: string | null }[]): string {
  const names = entries.map((e) => `${e.host}${e.path ?? ""}`);
  return names.length <= 2 ? names.join(", ") : `${names.slice(0, 2).join(", ")} and ${names.length - 2} more`;
}

/**
 * (C-963) The limits from above, one row each, with the value in words and who set it (`14 §8`). At the workspace level
 * every limit is shown, from the organization or the instance; at the organization level only what Administration sets
 * (the organization's own values are its controls, not limits).
 */
export function limitsFromAbove(
  level: "workspace" | "org",
  eff: EffectiveGovernance,
  who: { org: WhoCanValue; instance: WhoCanValue },
): LadderAbove[] {
  const rows: LadderAbove[] = [];
  const sourceOf = (s: string): "instance" | "org" => (s === "instance" ? "instance" : "org");
  const shown = (s: string) => level === "workspace" || s === "instance";
  const whoOf = (s: "instance" | "org") => (s === "instance" ? who.instance : who.org);
  const { posture, open_lane: lane, baseline_allowlist: base, org_allowlist: orgList } = eff.egress;
  if (shown(posture.source)) {
    const src = sourceOf(posture.source);
    rows.push({ label: "Websites agents may reach", value_display: POSTURE_WORDS[posture.value] ?? posture.value, source: src, who_can: whoOf(src) });
  }
  if (shown(lane.source)) {
    const src = sourceOf(lane.source);
    rows.push({ label: "Browser connections (not inspected)", value_display: LANE_WORDS[lane.value] ?? lane.value, source: src, who_can: whoOf(src) });
  }
  if (base.length > 0) rows.push({ label: "Allowed everywhere", value_display: listWords(base), source: "instance", who_can: who.instance });
  if (level === "workspace" && orgList.length > 0) {
    rows.push({ label: "Allowed in this organization", value_display: listWords(orgList), source: "org", who_can: who.org });
  }
  return rows;
}
