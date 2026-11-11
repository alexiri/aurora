"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { OrgDirectoryView } from "@/components/org/directory-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner"]}>
      {(org) => <OrgDirectoryView org={org} isOwner={org.my_role === "owner"} />}
    </OrgGate>
  );
}
