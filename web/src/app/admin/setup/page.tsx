"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { SetupView } from "@/components/admin/setup-view";

export default function AdminSetupPage() {
  return (
    <AdminGuard>
      <SetupView />
    </AdminGuard>
  );
}
