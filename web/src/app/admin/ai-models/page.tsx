"use client";

import { AdminAiModelsPage } from "@/components/admin/admin-topics";
import { LegacyGate } from "@/components/legacy-redirect";

/** `/admin/ai-models?tab=rules` is the old AI key rules tab: it moved to What agents may do › AI keys and rules. */
export default function Page() {
  return (
    <LegacyGate>
      <AdminAiModelsPage />
    </LegacyGate>
  );
}
