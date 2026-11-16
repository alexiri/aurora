"use client";

import { AdminAllSettingsPage } from "@/components/admin/admin-topics";
import { LegacyGate } from "@/components/legacy-redirect";

/** `/admin/settings/<section>` is storage, not navigation: it redirects to the topic page that holds the section (C-971). */
export default function Page() {
  return (
    <LegacyGate>
      <AdminAllSettingsPage />
    </LegacyGate>
  );
}
