"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { TrustedKeysView } from "@/components/admin/trusted-keys-view";

export default function Page() {
  return (
    <AdminGuard>
      <TrustedKeysView />
    </AdminGuard>
  );
}
