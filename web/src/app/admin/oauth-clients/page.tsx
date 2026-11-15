"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { PageHeader } from "@/components/ui/page-header";

export default function Page() {
  return (
    <AdminGuard>
      <div className="max-w-page">
        <PageHeader title="Instance OAuth clients" description="OAuth app registrations shared by every organization on this instance." />
        <OAuthClientsPanel scope={{ kind: "instance" }} canOwn />
      </div>
    </AdminGuard>
  );
}
