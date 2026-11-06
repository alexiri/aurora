"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { OrgKeysView } from "@/components/org/keys-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <OrgKeysView orgId={org.id} />}
    </OrgGate>
  );
}
