"use client";

import { useParams } from "next/navigation";

import { AdminGuard } from "@/components/admin/admin-guard";
import { PluginDetailView } from "@/components/admin/plugin-detail-view";

export default function Page() {
  const { id } = useParams<{ id: string }>();
  return (
    <AdminGuard>
      <PluginDetailView id={id} />
    </AdminGuard>
  );
}
