"use client";

import { useParams } from "next/navigation";

import { GroupsView } from "@/components/groups/groups-view";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <GroupsView orgId={org.id} orgSlug={slug} />}
    </OrgGate>
  );
}
