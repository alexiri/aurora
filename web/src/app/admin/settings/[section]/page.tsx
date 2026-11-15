"use client";

import { useParams } from "next/navigation";

import { AdminGuard } from "@/components/admin/admin-guard";
import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";

export default function AdminSettingsSectionPage() {
  const { section } = useParams<{ section: string }>();
  return (
    <AdminGuard>
      <SettingsSectionView slug={section} />
    </AdminGuard>
  );
}
