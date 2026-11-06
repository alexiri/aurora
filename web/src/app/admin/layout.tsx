import type { ReactNode } from "react";

import { AdminNav } from "@/components/admin/admin-guard";
import { AppShell } from "@/components/app-shell";

export default function AdminLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      <AdminNav />
      {children}
    </AppShell>
  );
}
