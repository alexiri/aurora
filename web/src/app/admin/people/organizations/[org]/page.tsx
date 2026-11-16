"use client";

import { useParams } from "next/navigation";

import { AdminGuard } from "@/components/admin/admin-guard";
import { OrgDetailView } from "@/components/admin/org-detail-view";

export default function AdminOrgPage() {
  const { org } = useParams<{ org: string }>();
  return (
    <AdminGuard>
      <OrgDetailView orgId={org} />
    </AdminGuard>
  );
}
