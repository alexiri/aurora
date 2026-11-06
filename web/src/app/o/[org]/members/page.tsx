"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { MembersView } from "@/components/org/members-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <MembersView orgId={org.id} viewer={org.my_role} />}
    </OrgGate>
  );
}
