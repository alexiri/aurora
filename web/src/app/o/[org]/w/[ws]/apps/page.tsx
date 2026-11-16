"use client";

import { LegacyGate } from "@/components/legacy-redirect";
import { WorkspaceAppsPage } from "@/components/workspace/workspace-topics";

export default function Page() {
  return (
    <LegacyGate>
      <WorkspaceAppsPage />
    </LegacyGate>
  );
}
