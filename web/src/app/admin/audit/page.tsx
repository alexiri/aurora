"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { AdminAuditView } from "@/components/admin/audit-view";

export default function Page() {
  return (
    <AdminGuard>
      <AdminAuditView />
    </AdminGuard>
  );
}
