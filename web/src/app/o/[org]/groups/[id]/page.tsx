"use client";

import { useParams } from "next/navigation";

import { GroupDetail } from "@/components/groups/group-detail";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug, id } = useParams<{ org: string; id: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <GroupDetail orgId={org.id} orgSlug={slug} id={id} />}
    </OrgGate>
  );
}
