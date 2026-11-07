"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { OrgSettingsView } from "@/components/org/settings-view";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <OrgSettingsView org={org} />}
    </OrgGate>
  );
}
