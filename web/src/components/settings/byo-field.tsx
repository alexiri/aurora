"use client";

import { Altitude } from "@/components/ui/altitude";
import { Field } from "@/components/ui/field";
import { Select } from "@/components/ui/select";
import { byoOptionsFrom, useEffectiveGovernance, type ByoPosture } from "@/lib/api/governance";
import type { Workspace } from "@/lib/api/queries";

/** What each posture says, as one sentence about who may create a bring-your-own agent. */
export const BYO_WORDS: Record<ByoPosture, string> = {
  enabled: "Members can create BYO agents",
  owners_only: "Only workspace owners can create BYO agents",
  disabled: "Nobody can create BYO agents",
};
/** The select's option labels: a native select cannot wrap, so they stay short; the help line carries the full sentence. */
const BYO_SHORT: Record<ByoPosture, string> = {
  enabled: "Members can create them",
  owners_only: "Only owners can create them",
  disabled: "No one can create them",
};
const words =(p: string): string => BYO_WORDS[p as ByoPosture] ?? p;
const lower = (s: string): string => s.charAt(0).toLowerCase() + s.slice(1);

type Above = { value: string; source: string };

/** The value in force, as the level above when the workspace has not tightened it (nothing otherwise). */
function fromEffective(w: Workspace): Above | null {
  const e = w.effective_byo_posture;
  return e && e.level !== "workspace" ? { value: e.value, source: e.level } : null;
}

/**
 * What the workspace's BYO field in Settings › General needs (C-967, D-442): the limit from above and whether the viewer
 * has anything to choose. Owners only; shown only when a stricter value than the level above exists, or when the
 * workspace has its own value that can be cleared. Owners read the organization's value (it stays visible once the
 * workspace has tightened it); the organization's governance is not read for anyone else.
 */
export function useByoChoice(workspace: Workspace, canOwn: boolean): { show: boolean; above: Above | null } {
  const isOwner = canOwn && workspace.my_role === "owner";
  const orgGov = useEffectiveGovernance(workspace.org_id, isOwner);
  if (!isOwner) return { show: false, above: null };
  const above = orgGov.data?.byo.posture ?? fromEffective(workspace);
  // The workspace's own value hides the level above until the organization's answer arrives: wait for it.
  if (!above && orgGov.isPending) return { show: false, above: null };
  const choice = above ? byoOptionsFrom(above.value).length > 1 : true;
  return { show: choice || !!workspace.byo_posture, above };
}

/** The field itself, a controlled select in the General form: `value` "" = the same as the level above. */
export function ByoField({ above, value, onChange, error }: { above: Above | null; value: string; onChange: (v: string) => void; error?: string }) {
  const sentence = above ? words(above.value) : null;
  const who = above?.source === "instance" ? "Aurora's administrators allow" : "Your organization allows";
  const options = byoOptionsFrom(above?.value ?? "enabled").filter((p) => p !== above?.value);
  return (
    <Altitude id="agent.byo" level="setup" promote={!!error}>
      <Field
        id="set-byo"
        label="Bring-your-own agents"
        hint={sentence ? `${who}: ${lower(sentence)}. A workspace can only be stricter.` : "A workspace can only be stricter than the level above."}
        error={error}>
        <Select
          id="set-byo"
          value={value}
          onChange={onChange}
          options={[
            { value: "", label: "Same as your organization" },
            ...options.map((p) => ({ value: p, label: BYO_SHORT[p as ByoPosture] ?? p })),
          ]}
        />
      </Field>
    </Altitude>
  );
}
