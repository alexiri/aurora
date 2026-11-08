import type { ReactNode } from "react";

import { AppShell } from "@/components/app-shell";
import { PageHeader } from "@/components/ui/page-header";

export default function AccountLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      <div className="max-w-[880px]">
        <PageHeader title="Account" description="Your profile, keys, sessions and linked channels." />
        {children}
      </div>
    </AppShell>
  );
}
