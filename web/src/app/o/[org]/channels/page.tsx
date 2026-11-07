"use client";

import { useParams } from "next/navigation";

import { ChannelsView } from "@/components/channels/channels-view";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <ChannelsView orgId={org.id} />}
    </OrgGate>
  );
}
