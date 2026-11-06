"use client";

import { useParams } from "next/navigation";

import { AvailabilityView } from "@/components/apps/availability-view";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin", "member"]}>
      {(org) => <AvailabilityView orgId={org.id} canEdit={org.my_role === "owner" || org.my_role === "admin"} />}
    </OrgGate>
  );
}
