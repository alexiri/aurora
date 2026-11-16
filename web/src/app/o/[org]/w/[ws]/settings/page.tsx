"use client";

import { LegacyGate } from "@/components/legacy-redirect";
import { WorkspaceSettingsPage } from "@/components/workspace/workspace-topics";

/** `?tab=access` is the old People address (C-960); every other tab is this page. */
export default function Page() {
  return (
    <LegacyGate>
      <WorkspaceSettingsPage />
    </LegacyGate>
  );
}
