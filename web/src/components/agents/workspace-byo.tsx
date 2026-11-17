"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ScopeLadder, type LadderAbove } from "@/components/ui/scope-ladder";
import { usePatchWorkspace } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { byoOptionsFrom, useEffectiveGovernance, type ByoPosture } from "@/lib/api/governance";
import type { Workspace } from "@/lib/api/queries";
import { whoCanFromContacts } from "@/lib/who-can";

/** What each posture says, as one sentence about who may create a bring-your-own agent. */
const BYO_WORDS: Record<ByoPosture, string> = {
  enabled: "Members can create BYO agents",
  owners_only: "Only workspace owners can create BYO agents",
  disabled: "Nobody can create BYO agents",
};
const words = (p: string): string => BYO_WORDS[p as ByoPosture] ?? p;

/**
 * The workspace's BYO setting (C-967), in the header of its agents list (it was Settings › General): the value from
 * above as a ladder row, and the workspace's own choice behind **Tighten…**. A workspace can only tighten what the
 * organization or the instance allows. Viewers who are not owners read the value as text.
 */
export function WorkspaceByoRow({ workspace }: { workspace: Workspace }) {
  return workspace.my_role === "owner" ? <OwnerByoRow workspace={workspace} /> : <ByoRow workspace={workspace} above={fromEffective(workspace)} />;
}

type Above = { value: string; source: string };

/** The value in force, as the level above when the workspace has not tightened it (nothing otherwise). */
function fromEffective(w: Workspace): Above | null {
  const e = w.effective_byo_posture;
  return e && e.level !== "workspace" ? { value: e.value, source: e.level } : null;
}

/** Owners read the organization's own value, which stays visible once the workspace has tightened it. */
function OwnerByoRow({ workspace }: { workspace: Workspace }) {
  const orgGov = useEffectiveGovernance(workspace.org_id);
  return <ByoRow workspace={workspace} above={orgGov.data?.byo.posture ?? fromEffective(workspace)} />;
}

function ByoRow({ workspace, above }: { workspace: Workspace; above: Above | null }) {
  const canOwn = workspace.my_role === "owner";
  const patch = usePatchWorkspace(workspace.id);
  const own = workspace.byo_posture ?? "";
  const [choice, setChoice] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const effective = workspace.effective_byo_posture;
  if (!above) return null;
  const source: LadderAbove["source"] = above.source === "instance" ? "instance" : "org";
  const rows: LadderAbove[] = [
    {
      label: "Bring-your-own agents",
      value_display: words(above.value),
      source,
      who_can: whoCanFromContacts(source === "instance" ? "instance_admin" : "org_admin", undefined, "change_this"),
    },
  ];
  const value = choice ?? own;
  const fe = patch.error ? fieldErrors(patch.error) : {};
  return (
    <div data-testid="workspace-byo" className="space-y-2">
      <ScopeLadder
        above={rows}
        collapsed
        here={
          canOwn ? (
            <div className="max-w-md space-y-3">
              <Field
                id="ws-byo"
                label="In this workspace"
                hint="A workspace can only be stricter than the organization, never looser."
                error={fe.byo_posture}
              >
                <select
                  id="ws-byo"
                  className={inputClass}
                  value={value}
                  disabled={patch.isPending}
                  onChange={(e) => {
                    setChoice(e.target.value);
                    setSaved(false);
                  }}
                >
                  <option value="">Same as the organization ({words(above.value)})</option>
                  {byoOptionsFrom(above.value)
                    .filter((p) => p !== above.value)
                    .map((p) => (
                      <option key={p} value={p}>
                        {words(p)}
                      </option>
                    ))}
                </select>
              </Field>
              {patch.error && Object.keys(fe).length === 0 ? (
                <Notice tone="danger" role="alert">
                  {errorMessage(patch.error)}
                </Notice>
              ) : null}
              {saved ? (
                <Notice tone="accent" role="status">
                  Saved.
                </Notice>
              ) : null}
              <Button
                type="button"
                disabled={patch.isPending || value === own}
                onClick={() => patch.mutate({ byo_posture: value || null }, { onSuccess: () => { setSaved(true); setChoice(null); } })}
              >
                {patch.isPending ? "Saving…" : "Save"}
              </Button>
            </div>
          ) : undefined
        }
      />
      {effective && effective.level === "workspace" ? (
        <p className="text-sm text-ink-2">This workspace is stricter: {words(effective.value).toLowerCase()}.</p>
      ) : null}
    </div>
  );
}
