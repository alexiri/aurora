import type { ReactNode } from "react";

import { AccountNav } from "@/components/account/account-nav";
import { AppShell } from "@/components/app-shell";

export default function AccountLayout({ children }: { children: ReactNode }) {
  return (
    <AppShell>
      <div className="max-w-3xl">
        <h1 className="mb-4 text-xl font-semibold">Account</h1>
        <AccountNav />
        {children}
      </div>
    </AppShell>
  );
}
