"use client";

import { useState } from "react";

import { GovernanceFields } from "@/components/admin/settings/governance-settings-view";
import { PolicyPresets } from "@/components/policy/policy-presets";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { ScopeLadder } from "@/components/ui/scope-ladder";
import { Section } from "@/components/ui/section";
import { useGovernanceOrgs } from "@/lib/api/settings";
import { orgRoutes } from "@/lib/routes";

const POSTURE = { open: "Any website", credentialed_only: "Only listed websites" } as Record<string, string>;
const LANE = { enabled: "Allowed", allowlist_only: "Only listed websites", disabled: "Turned off" } as Record<string, string>;

/** (C-964) Where organizations stand on Internet access: what each one tightened, from `GET …/governance/orgs`. */
function Organizations() {
  const [cursors, setCursors] = useState<string[]>([]);
  const q = useGovernanceOrgs(cursors[cursors.length - 1]);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const rows = q.data.items.map((o) => ({
    label: o.name,
    values: [
      POSTURE[o.egress_posture] ?? o.egress_posture,
      LANE[o.open_lane] ?? o.open_lane,
      o.stricter.some((k) => k.startsWith("governance.egress.")) ? "Yes" : "No",
    ],
    link: { href: orgRoutes(o.slug).internetAccess(), label: "Open" },
  }));
  return (
    <Section id="internet-organizations" title="Where organizations stand">
      <ScopeLadder
        above={[]}
        below={{ columns: ["Websites", "Browser connections", "Stricter than the limits above"], rows, empty: "No organizations yet." }}
      />
      <div className="mt-2 flex flex-wrap gap-2">
        {cursors.length > 0 ? (
          <Button variant="outline" size="sm" onClick={() => setCursors((c) => c.slice(0, -1))}>
            Previous
          </Button>
        ) : null}
        {q.data.next_cursor ? (
          <Button variant="outline" size="sm" onClick={() => setCursors((c) => [...c, q.data.next_cursor as string])}>
            Next
          </Button>
        ) : null}
      </div>
    </Section>
  );
}

/** (P5-IA-2, C-965) Administration's Internet access: the limits no organization can go below, and where organizations stand. */
export function AdminInternet() {
  return (
    <div className="space-y-8">
      <PageHeader title="What agents may do" description="Which websites agents can reach. Organizations and workspaces can only restrict further what you set here." className="mb-2" />
      {/* (P5-REFINE-18 #2, D-410) The preset first; today's fields behind Customise. */}
      <PolicyPresets>
        <GovernanceFields part="internet" />
      </PolicyPresets>
      <Organizations />
    </div>
  );
}
