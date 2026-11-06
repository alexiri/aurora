"use client";

import { useParams } from "next/navigation";

import { CredentialsView } from "@/components/credentials/credentials-view";
import { OrgGate } from "@/components/org/org-gate";

export default function Page() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <CredentialsView
          title="Organization credentials"
          scope={{ kind: "organization", id: org.id }}
          orgId={org.id}
          canOwn
        />
      )}
    </OrgGate>
  );
}
