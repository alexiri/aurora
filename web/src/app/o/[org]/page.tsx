"use client";

import { useParams } from "next/navigation";

import { OrgOverview } from "@/components/org/org-overview";

export default function Page() {
  const { org } = useParams<{ org: string }>();
  return <OrgOverview slug={org} />;
}
