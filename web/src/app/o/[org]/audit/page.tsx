"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { OrgAuditView } from "@/components/org/audit-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <OrgAuditView orgId={org.id} />}
    </OrgGate>
  );
}
