"use client";

import { grantSummary } from "@/components/identity/idp-roles-list";
import { PROVIDERS } from "@/components/admin/settings/sign-in-settings-view";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { useSettingsSection } from "@/lib/api/settings";
import { useSession } from "@/lib/api/queries";
import type { IdpGrant } from "@/components/identity/idp-roles-list";

type Entry = Record<string, unknown>;
const text = (v: unknown): string => (typeof v === "string" ? v : "");

/** What an entry of `role_mappings` grants, in the shape the claim debugger already words. */
function toGrant(m: Entry): IdpGrant {
  const org = m.org as { slug?: unknown; role?: unknown } | undefined;
  const workspaces = Array.isArray(m.workspaces) ? (m.workspaces as { slug?: unknown; role?: unknown }[]) : [];
  return {
    value: text(m.value),
    instance_admin: m.instance_admin === true,
    org: org && text(org.slug) ? { slug: text(org.slug), role: text(org.role) || "member" } : null,
    workspaces: workspaces.map((w) => ({ slug: text(w.slug), role: text(w.role) || "member" })),
  } as IdpGrant;
}

/**
 * People › Sign-in, "Roles from the provider" (P5-IA-4, C-968): what the Identity page showed, read from the
 * providers of the `sign_in` section: the claim each provider's roles come from, which value gives which role, and
 * what the viewer's last sign-in made of them. Changing a mapping is done on the provider (Edit › Roles).
 */
export function ProviderRolesView() {
  const q = useSettingsSection("sign_in");
  const me = useSession();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const setting = q.data.settings.find((s) => s.key === PROVIDERS);
  const providers = (Array.isArray(setting?.value) ? (setting.value as Entry[]) : []).filter((p) => text(p.id) !== "");
  const mine = me.data?.idp_roles;
  const myProvider = mine ? providers.find((p) => p.id === mine.provider) : undefined;
  if (providers.length === 0) {
    return <p className="text-sm text-ink-2">No provider yet, so there are no roles to map. Add one above.</p>;
  }
  return (
    <div className="space-y-6">
      {providers.map((p) => {
        const id = text(p.id);
        const name = text(p.display_name) || id;
        const claim = text(p.role_claim);
        const mappings = (Array.isArray(p.role_mappings) ? (p.role_mappings as Entry[]) : []).map(toGrant);
        return (
          <div key={id} className="space-y-2" aria-label={`Roles from ${name}`} role="group">
            <h3 className="text-sm font-semibold text-ink">{name}</h3>
            {claim ? (
              <>
                <p className="text-sm text-ink-2">
                  The <code className="font-mono">{claim}</code> claim of the token decides who is an Aurora administrator and which
                  organization people join.{" "}
                  {p.require_role === true ? "People whose roles map to nothing cannot sign in." : "Everyone the provider admits may sign in."}
                </p>
                {mappings.length === 0 ? (
                  <p className="text-sm text-ink-2">Nothing is mapped yet: role values are shown to people but give them nothing.</p>
                ) : (
                  <DataTable caption={`Roles from ${name}`}>
                    <thead>
                      <tr>
                        <Th>Role value</Th>
                        <Th>Gives</Th>
                      </tr>
                    </thead>
                    <tbody>
                      {mappings.map((m) => (
                        <tr key={m.value}>
                          <Td>
                            <code className="font-mono text-xs [overflow-wrap:anywhere]">{m.value}</code>
                          </Td>
                          <Td>
                            <div className="flex flex-wrap gap-1.5">
                              {grantSummary(m).map((g) => (
                                <Badge key={g} tone={g === "Instance admin" ? "accent" : "neutral"} className="normal-case tracking-normal">
                                  {g === "Instance admin" ? "Aurora administrator" : g}
                                </Badge>
                              ))}
                            </div>
                          </Td>
                        </tr>
                      ))}
                    </tbody>
                  </DataTable>
                )}
              </>
            ) : (
              <p className="text-sm text-ink-2">This provider does not send role values. Roles are set in Aurora.</p>
            )}
          </div>
        );
      })}
      {mine && myProvider ? (
        <p className="text-sm text-ink-2" data-testid="my-roles">
          You signed in through {text(myProvider.display_name) || mine.provider} with{" "}
          {mine.values.length > 0 ? mine.values.map((v, i) => (
            <span key={v}>
              {i > 0 ? ", " : ""}
              <code className="font-mono">{v}</code>
            </span>
          )) : "no role values"}
          {mine.granted.length > 0 ? `: that makes you ${mine.granted.flatMap(grantSummary).map((g) => (g === "Instance admin" ? "an Aurora administrator" : g)).join(" and ")}.` : ". None of them gives you anything here."}
        </p>
      ) : null}
    </div>
  );
}
