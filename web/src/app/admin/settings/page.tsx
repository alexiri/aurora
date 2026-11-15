"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { SettingsIndexView } from "@/components/admin/settings/settings-index-view";

export default function AdminSettingsPage() {
  return (
    <AdminGuard>
      <SettingsIndexView />
    </AdminGuard>
  );
}
