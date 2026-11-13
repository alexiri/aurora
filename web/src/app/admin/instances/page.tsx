"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { InstancesView } from "@/components/apps/instances-view";
import { PageHeader } from "@/components/ui/page-header";

export default function Page() {
  return (
    <AdminGuard>
      <div className="max-w-5xl">
        <PageHeader
          title="Self-hosted apps"
          description="Your own GitLab, GitHub Enterprise Server, Indico … so people can connect to it: every registration, with its organization."
        />
        <InstancesView scope={{ kind: "instance", all: true }} canManage />
      </div>
    </AdminGuard>
  );
}
