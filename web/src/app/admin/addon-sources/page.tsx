"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { AddonSourcesView } from "@/components/admin/addon-sources-view";

export default function Page() {
  return (
    <AdminGuard>
      <AddonSourcesView />
    </AdminGuard>
  );
}
