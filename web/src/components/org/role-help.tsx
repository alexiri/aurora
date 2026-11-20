"use client";

import { ROLE_CAN, type OrgRoleName } from "@/lib/roles-explain";

const ORDER: OrgRoleName[] = ["member", "admin", "owner"];

/** (org-9) "What can each role do?" as a fold, the same wording as the promotion dialog (`ROLE_CAN`). */
export function OrgRoleHelp({ open }: { open?: boolean }) {
  return (
    <details className="text-sm text-ink-2" open={open}>
      <summary className="cursor-pointer font-medium text-ink">What can each role do?</summary>
      <dl aria-label="Roles" className="mt-2 space-y-2">
        {ORDER.map((r) => (
          <div key={r}>
            <dt className="font-medium text-ink">{ROLE_CAN.org[r].label}</dt>
            <dd>
              {ROLE_CAN.org[r].summary} {ROLE_CAN.org[r].can.join(" · ")}.
            </dd>
          </div>
        ))}
      </dl>
    </details>
  );
}
