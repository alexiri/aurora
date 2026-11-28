"use client";

import { Errors, Locked, PostureSelect } from "@/components/governance/controls";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { ItemList, ListRow } from "@/components/ui/section";
import {
  issuesUnder,
  useBlockedHosts,
  type EffectiveGovernance,
  type GovForm,
  type GovIssues,
} from "@/lib/api/governance";
import { useWorkspaces } from "@/lib/api/queries";
import { isEmergencyReader } from "@/lib/emergency";
import { cn } from "@/lib/utils";

/** The organization's Internet access controls of the governance document (C-965): postures, allow-list, blocked hosts. */
export function EgressTab({ form, patch, issues, readOnly, eff, orgId }: {
  form: GovForm; patch: (p: Partial<GovForm>) => void; issues: GovIssues; readOnly: boolean; eff: EffectiveGovernance; orgId: string;
}) {
  const workspaces = useWorkspaces(orgId);
  // An administrator's list also holds workspaces they aren't in, whose activity they can't read: don't ask for those.
  const all = workspaces.data?.items ?? [];
  const readable = all.filter((w) => w.my_role || isEmergencyReader(w));
  const blocked = useBlockedHosts(readable.map((w) => w.id), true);
  const setRow = (i: number, p: Partial<{ host: string; path: string }>) =>
    patch({ allowlist: form.allowlist.map((r, j) => (j === i ? { ...r, ...p } : r)) });
  const listed = new Set(form.allowlist.map((r) => r.host));
  return (
    <>
      <PostureSelect
        label="Which websites agents may reach"
        value={form.egressPosture}
        locked={form.egressPostureLocked}
        setting="governance.egress.minimum_posture"
        readOnly={readOnly}
        onChange={(v) => patch({ egressPosture: v })}
        options={[
          { value: "open", label: "Any website: except what is blocked above and by your rules" },
          { value: "credentialed_only", label: "Only listed websites: agents reach their accounts' apps and the allow-list" },
        ]}
      />
      <Errors messages={issuesUnder(issues, "/egress/posture")} />
      <PostureSelect
        label="Browser connections (not inspected)"
        value={form.openLane}
        locked={form.openLaneLocked}
        setting="governance.egress.open_lane_minimum"
        readOnly={readOnly}
        onChange={(v) => patch({ openLane: v })}
        options={[
          { value: "enabled", label: "Allowed: browsers may connect to any website" },
          { value: "allowlist_only", label: "Only listed websites: browsers reach only the allow-list" },
          { value: "disabled", label: "Turned off: no browser connections" },
        ]}
      />
      <p className="text-xs text-muted">Aurora can&apos;t see inside these or give them an account, so the allow-list is the only limit.</p>
      <Errors messages={issuesUnder(issues, "/egress/open_lane")} />
      <section aria-label="Organization allow-list" className="space-y-3">
        <h2 className="font-display text-base font-semibold text-ink">Organization allow-list</h2>
        {form.allowlist.length === 0 ? <p className="text-sm text-ink-2">No hosts allowed beyond the instance baseline.</p> : null}
        {form.allowlist.map((r, i) => (
          <div key={i} className="space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <input aria-label={`Host ${i + 1}`} className={cn(inputClass, "min-w-0 flex-1 basis-48")} placeholder="host, e.g. *.example.com" value={r.host} disabled={readOnly} onChange={(e) => setRow(i, { host: e.target.value })} />
              <input aria-label={`Path ${i + 1}`} className={cn(inputClass, "min-w-0 flex-1 basis-48")} placeholder="path pattern (optional)" value={r.path} disabled={readOnly} onChange={(e) => setRow(i, { path: e.target.value })} />
              {!readOnly ? (
                <Button variant="ghost" size="sm" onClick={() => patch({ allowlist: form.allowlist.filter((_, j) => j !== i) })}>
                  Remove
                </Button>
              ) : null}
            </div>
            <Errors messages={issuesUnder(issues, `/egress/allowlist/${i}`)} />
          </div>
        ))}
        <Errors messages={issuesUnder(issues, "/egress/allowlist").filter((m) => !/^\d+/.test(m))} />
        {!readOnly ? (
          <Button variant="outline" size="sm" onClick={() => patch({ allowlist: [...form.allowlist, { host: "", path: "" }] })}>
            Add website
          </Button>
        ) : null}
      </section>
      {blocked.length > 0 ? (
        <section aria-label="Recently blocked hosts" className="space-y-3">
          <h2 className="font-display text-base font-semibold text-ink">Recently blocked hosts</h2>
          {readable.length < all.length ? <p className="text-xs text-muted">From the workspaces you can read.</p> : null}
          <ItemList aria-label="Recently blocked hosts">
            {blocked.map((b) => (
              <ListRow
                key={b.host}
                actions={
                  !readOnly && !listed.has(b.host) ? (
                    <Button variant="ghost" size="sm" onClick={() => patch({ allowlist: [...form.allowlist, { host: b.host, path: "" }] })}>
                      Add to allow-list
                    </Button>
                  ) : null
                }
              >
                <span className="font-mono text-sm text-ink">{b.host}</span>
                <span className="text-xs text-muted">{b.count}x</span>
              </ListRow>
            ))}
          </ItemList>
        </section>
      ) : null}
      <section aria-label="Instance baseline" className="space-y-3">
        <h2 className="font-display text-base font-semibold text-ink">Instance baseline<Locked setting="governance.egress.baseline_allowlist" /></h2>
        {eff.egress.baseline_allowlist.length === 0 ? (
          <p className="text-sm text-ink-2">The instance has no baseline allow-list.</p>
        ) : (
          <ul className="rounded-card border border-line bg-surface px-4 py-3 font-mono text-sm text-ink shadow-card">
            {eff.egress.baseline_allowlist.map((e, i) => (
              <li key={i} className="break-all">{e.host}{e.path ?? ""}</li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}
