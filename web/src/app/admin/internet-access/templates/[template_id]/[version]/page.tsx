"use client";

import { useParams } from "next/navigation";

import { AdminGuard } from "@/components/admin/admin-guard";
import { TemplateVersionReview } from "@/components/templates/review-version";

/** The review of one agent-template version (C-1114). */
export default function Page() {
  const { template_id, version } = useParams<{ template_id: string; version: string }>();
  return (
    <AdminGuard>
      <TemplateVersionReview templateId={template_id} version={version} />
    </AdminGuard>
  );
}
