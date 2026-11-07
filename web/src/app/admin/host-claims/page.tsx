"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { HostClaimsView } from "@/components/admin/host-claims-view";

export default function Page() {
  return (
    <AdminGuard>
      <HostClaimsView />
    </AdminGuard>
  );
}
