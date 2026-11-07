"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";

export default function Page() {
  return (
    <AdminGuard>
      <ConnectorsAdminView />
    </AdminGuard>
  );
}
