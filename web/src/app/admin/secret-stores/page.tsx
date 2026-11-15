"use client";

import { AdminGuard } from "@/components/admin/admin-guard";
import { SecretStoresView } from "@/components/admin/secret-stores-view";

export default function AdminSecretStoresPage() {
  return (
    <AdminGuard>
      <SecretStoresView />
    </AdminGuard>
  );
}
