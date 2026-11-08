"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { InstancesView } from "@/components/apps/instances-view";
import { PageHeader } from "@/components/ui/page-header";

export default function Page() {
  return (
    <AdminGuard>
      <div className="max-w-5xl">
        <PageHeader
          title="Instances"
          description="Self-hosted deployments of apps that every organization can connect to."
        />
        <InstancesView scope={{ kind: "instance" }} canManage />
      </div>
    </AdminGuard>
  );
}
