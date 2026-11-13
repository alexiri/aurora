"use client";

import { useParams } from "next/navigation";

import { AvailabilityView } from "@/components/apps/availability-view";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <AvailabilityView orgId={org.id} orgSlug={org.slug} canEdit />}
    </OrgGate>
  );
}
