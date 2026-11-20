"use client";

import { AdminInternetAccessPage } from "@/components/admin/admin-topics";
import { LegacyGate } from "@/components/legacy-redirect";

/** `/admin/internet-access?tab=limits` was the Internet access limits: that is now the Internet access tab. */
export default function Page() {
  return (
    <LegacyGate>
      <AdminInternetAccessPage />
    </LegacyGate>
  );
}
