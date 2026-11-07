import type { ReactNode } from "react";

import { AppShell } from "@/components/app-shell";
import { PageHeader } from "@/components/ui/page-header";

export default function AccountLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      <div className="max-w-3xl">
        <PageHeader title="Account" />
        {children}
      </div>
    </AppShell>
  );
}
