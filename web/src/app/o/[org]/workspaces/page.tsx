"use client";

import { useParams } from "next/navigation";

import { OrgWorkspaces } from "@/components/org/org-workspaces";

/** (D-508(c), C-1178) Manage <org> › Workspaces (`/o/<org>/workspaces`). */
export default function Page() {
  const { org } = useParams<{ org: string }>();
  return <OrgWorkspaces slug={org} />;
}
