"use client";

import { useParams } from "next/navigation";

import { InstancesView } from "@/components/apps/instances-view";
import { OrgGate } from "@/components/org/org-gate";
import { PageHeader } from "@/components/ui/page-header";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div className="max-w-5xl">
          <PageHeader
            title="Instances"
            description="Self-hosted deployments of apps, such as your own GitLab."
            breadcrumb={[{ label: org.name, href: `/o/${slug}` }, { label: "Instances" }]}
          />
          <InstancesView scope={{ kind: "organization", id: org.id }} canManage />
        </div>
      )}
    </OrgGate>
  );
}
