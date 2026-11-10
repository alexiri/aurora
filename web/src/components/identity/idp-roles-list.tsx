import { fmtDate } from "@/components/account/key-table";
import type { components } from "@/lib/api/schema";

export type IdpRoles = components["schemas"]["IdpRoles"];
export type IdpGrant = components["schemas"]["IdpGrant"];

/** What one matched claim value granted, as short phrases. */
export function grantSummary(g: IdpGrant): string[] {
  const out: string[] = [];
  if (g.instance_admin) out.push("Instance admin");
  if (g.org) out.push(`${g.org.role} in organization ${g.org.slug}`);
  for (const w of g.workspaces) out.push(`${w.role} in workspace ${g.org?.slug ?? "?"}/${w.slug}`);
  return out;
}

/**
 * The claim debugger (P4-IDP-1, D-139): every role value of the last sign-in and what it
 * granted. Values that map to nothing are listed too, which is what an operator needs when
 * "it doesn't work".
 */
export function IdpRolesList({ roles }: { roles: IdpRoles }) {
  const granted = new Map(roles.granted.map((g) => [g.value, g]));
  if (roles.values.length === 0) {
    return <p className="text-sm text-ink-2">The sign-in carried no role values.</p>;
  }
  return (
    <div className="space-y-2">
      <ul className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface text-sm">
        {roles.values.map((v) => {
          const g = granted.get(v);
          const what = g ? grantSummary(g) : [];
          return (
            <li key={v} className="flex flex-col gap-0.5 px-3 py-2 sm:flex-row sm:items-baseline sm:gap-4">
              <code className="font-mono text-xs text-ink sm:w-56 sm:shrink-0 [overflow-wrap:anywhere]">{v}</code>
              <span className={what.length > 0 ? "text-ink-2" : "text-muted"}>
                {what.length > 0 ? what.join("; ") : "Grants nothing in Aurora"}
              </span>
            </li>
          );
        })}
      </ul>
      <p className="text-xs text-muted">
        Read from {roles.provider} at your last sign-in ({fmtDate(roles.synced_at)}). Changes in the identity
        provider apply at your next sign-in.
      </p>
    </div>
  );
}
