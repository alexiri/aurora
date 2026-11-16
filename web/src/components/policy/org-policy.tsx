"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { useAllMembers } from "@/lib/api/org";

import { PolicyView } from "./policy-view";

function Inner({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const members = useAllMembers(orgId);
  const users = (members.data?.pages ?? []).flatMap((p) => p.items).map((m) => ({ id: m.user_id, label: `${m.display_name} (${m.email})` }));
  return <PolicyView title="Organization policy" scope={{ kind: "organization", id: orgId }} orgId={orgId} orgSlug={orgSlug} canEdit users={users} />;
}

export function OrgPolicy() {
  const { org: slug } = useParams<{ org: string }>();
  return <OrgGate slug={slug} allow={["owner", "admin"]}>{(org) => <Inner orgId={org.id} orgSlug={slug} />}</OrgGate>;
}
