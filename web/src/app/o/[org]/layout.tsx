import type { ReactNode } from "react";

import { AppShell } from "@/components/app-shell";
import { ManageFrame } from "@/components/org/manage-frame";

export default function OrgLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      {/* (P5-REFINE-17, rec 8) The "Manage <org>" area around the organization's pages; workspace pages pass through. */}
      <ManageFrame>{children}</ManageFrame>
    </AppShell>
  );
}
