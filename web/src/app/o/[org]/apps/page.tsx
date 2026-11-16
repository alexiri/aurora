"use client";

import { OrgAppsPage } from "@/components/org/org-topics";
import { LegacyGate } from "@/components/legacy-redirect";

/** `/o/<org>/apps` without a tab is the old catalogue address: it redirects to the Apps tab (C-960). */
export default function Page() {
  return (
    <LegacyGate>
      <OrgAppsPage />
    </LegacyGate>
  );
}
