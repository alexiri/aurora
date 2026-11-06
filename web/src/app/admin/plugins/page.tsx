"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { PluginsView } from "@/components/admin/plugins-view";

export default function Page() {
  return (
    <AdminGuard>
      <PluginsView />
    </AdminGuard>
  );
}
