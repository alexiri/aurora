"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { AdminRunnersView } from "@/components/admin/runners-view";

export default function AdminRunnersPage() {
  return (
    <AdminGuard>
      <AdminRunnersView />
    </AdminGuard>
  );
}
