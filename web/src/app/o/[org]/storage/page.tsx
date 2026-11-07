"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { OrgStorageView } from "@/components/org/storage-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <OrgStorageView orgId={org.id} />}
    </OrgGate>
  );
}
