"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";

export default function Page() {
  return (
    <AdminGuard>
      <div className="max-w-4xl space-y-4">
        <h1 className="text-xl font-semibold">Instance OAuth clients</h1>
        <OAuthClientsPanel scope={{ kind: "instance" }} canOwn />
      </div>
    </AdminGuard>
  );
}
