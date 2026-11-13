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
            title="Self-hosted apps"
            description="Register your own GitLab, GitHub Enterprise Server, Indico … so people can connect to it."
            breadcrumb={[{ label: org.name, href: `/o/${slug}` }, { label: "Self-hosted apps" }]}
          />
          <InstancesView scope={{ kind: "organization", id: org.id }} canManage />
        </div>
      )}
    </OrgGate>
  );
}
