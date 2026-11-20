"use client";

import { useState } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { MenuSwitchItem } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { useAccess } from "@/lib/api/account";
import { useReassignCreator, type Offboarding, type ReassignTarget } from "@/lib/api/automations";

type Creator = { id: string; display_name: string } | null | undefined;

/** "Owner: Alice" for creator-anchored items, "Owner: Workspace" for workspace-anchored ones. */
export function OwnerLabel({ anchor, creator }: { anchor: string; creator: Creator }) {
  if (anchor === "workspace") {
    return (
      <span className="text-xs text-ink-2" title={creator ? `Created by ${creator.display_name}` : undefined}>
        Owner: <strong className="font-medium">Workspace</strong>
      </span>
    );
  }
  return (
    <span className="text-xs text-ink-2">
      Owner: {creator ? creator.display_name || "Unknown" : "nobody (creator removed)"}
    </span>
  );
}

/**
 * (P5-REFINE-14) Workspace owners only, in the row's "…" menu: **Keep running without you** (a switch row). Anchoring makes
 * the automation outlive any single member: it keeps running as long as the workspace has an active owner. Expert
 * (P5-REFINE-3): without the person's "Show more controls" the item is not in the menu.
 */
export function AnchorMenuItem({
  name,
  anchor,
  busy,
  onChange,
}: {
  name: string;
  anchor: string;
  busy?: boolean;
  onChange: (anchor: "creator" | "workspace") => void;
}) {
  return (
    <Altitude id="auto.anchor" level="expert">
      <MenuSwitchItem checked={anchor === "workspace"} disabled={busy} onCheckedChange={(on) => onChange(on ? "workspace" : "creator")}>
        <span className="sr-only">{name}: </span>Keep running without you
      </MenuSwitchItem>
    </Altitude>
  );
}

/** "Reassign…": pick a member who can use the workspace; optionally re-enable. */
export function ReassignPanel({
  agentId,
  wsId,
  kind,
  id,
  name,
  creatorId,
  disabledReason,
  onClose,
}: {
  agentId: string;
  wsId: string;
  kind: "schedule" | "webhook";
  id: string;
  name: string;
  creatorId?: string;
  disabledReason?: string | null;
  onClose: () => void;
}) {
  const access = useAccess(wsId);
  const reassign = useReassignCreator(agentId, kind);
  const [user, setUser] = useState("");
  const [reEnable, setReEnable] = useState(disabledReason === "authorization");
  const candidates = (access.data?.items ?? []).filter((b) => b.user_id !== creatorId);
  return (
    <form
      aria-label={`Reassign ${name}`}
      className="space-y-3 rounded-card border border-line bg-subtle p-3 sm:p-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!user) return;
        reassign.mutate(
          { id, user_id: user, re_enable: disabledReason === "authorization" ? reEnable : undefined },
          { onSuccess: onClose },
        );
      }}
    >
      <label htmlFor={`reassign-${id}`} className="block text-sm font-medium">
        New owner
      </label>
      <select id={`reassign-${id}`} className={inputClass} value={user} onChange={(e) => setUser(e.target.value)}>
        <option value="">Select a member…</option>
        {candidates.map((b) => (
          <option key={b.user_id} value={b.user_id}>
            {b.display_name} ({b.email})
          </option>
        ))}
      </select>
      {disabledReason === "authorization" ? (
        <CheckboxField
          id={`reassign-re-enable-${id}`}
          label="Re-enable it (it was disabled because its owner lost access)"
          checked={reEnable}
          onChange={setReEnable}
        />
      ) : null}
      <ErrorNotice error={reassign.error} />
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={!user || reassign.isPending}>
          Reassign
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onClose}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/**
 * The offboarding prompt: what the removal would disable and where to reassign it. `who` is the
 * person's name; `candidates` are the people who can take the items over.
 */
export function OffboardingPrompt({
  who,
  data,
  candidates,
  target,
  onTarget,
}: {
  who: string;
  data: Offboarding;
  candidates: { id: string; label: string }[];
  target: ReassignTarget | "";
  onTarget: (t: ReassignTarget | "") => void;
}) {
  const items = [...data.schedules, ...data.webhooks];
  if (items.length === 0 && data.peer_tasks.length === 0 && data.api_keys === 0) return null;
  return (
    <div className="space-y-2 text-sm">
      {items.length > 0 ? (
        <>
          <p>
            Removing {who} disables these {items.length} schedules and webhooks — reassign?
          </p>
          <ul className="max-h-40 list-disc overflow-y-auto pl-5">
            {items.map((i) => (
              <li key={i.id}>
                {i.name} <span className="text-ink-2">({i.agent.name}, {i.workspace.name})</span>
              </li>
            ))}
          </ul>
          <label htmlFor="offboard-target" className="block font-medium">
            Reassign to
          </label>
          <select
            id="offboard-target"
            className={inputClass}
            value={target}
            onChange={(e) => onTarget(e.target.value as ReassignTarget | "")}
          >
            <option value="">Nobody — let them be disabled</option>
            <option value="workspace">Workspace (keeps running while a workspace owner is active)</option>
            {candidates.map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </>
      ) : null}
      {data.peer_tasks.length > 0 ? (
        <p className="text-ink-2">{data.peer_tasks.length} open peer task(s) they requested will stop.</p>
      ) : null}
      {data.api_keys > 0 ? <p className="text-ink-2">{data.api_keys} API key(s) will stop working.</p> : null}
      {(data.byo_agents_created ?? 0) > 0 ? (
        <p className="text-ink-2">
          {data.byo_agents_created} BYO agent{(data.byo_agents_created ?? 0) === 1 ? "" : "s"}{" "}
          they created &mdash; regenerate their tokens: removal takes the agents&apos;
          credentials with them.
        </p>
      ) : null}
    </div>
  );
}
