import type { ReactNode } from "react";

import { AppShell } from "@/components/app-shell";
import { ManageGate } from "@/components/org/manage-gate";

export default function OrgLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      {/* (D-508) Manage <org> is a place: the shell's rail is its menu. Members are sent to their workspace;
          workspace pages pass through. */}
      <ManageGate>{children}</ManageGate>
    </AppShell>
  );
}
