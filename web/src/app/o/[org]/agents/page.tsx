"use client";

import { LegacyGate } from "@/components/legacy-redirect";
import { OrgAgentsPage } from "@/components/org/org-topics";

/** (D-506(a)) `?tab=skills` moved to Manage <org> › Skills; `LegacyGate` replaces that address. */
export default function Page() {
  return (
    <LegacyGate>
      <OrgAgentsPage />
    </LegacyGate>
  );
}
