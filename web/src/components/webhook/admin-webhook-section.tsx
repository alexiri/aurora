"use client";

import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";
import { useSettingsIndex } from "@/lib/api/settings";

/** The settings section C-913 adds; the server lists it once P5-WEBHOOK-2 is deployed. */
export const OUTGOING_WEBHOOKS_SECTION = "outgoing_webhooks";

/**
 * Administration › Apps & accounts › Advanced: the `outgoing_webhooks` section through the generic form. Gated on the
 * server's settings index listing it: for a slug it does not know `SettingsSectionView` would show a not-found error, so
 * until P5-WEBHOOK-2 is deployed (or while the index loads or fails) nothing renders here.
 */
export function OutgoingWebhooksAdminSection() {
  const index = useSettingsIndex();
  if (!index.data?.sections.some((s) => s.section === OUTGOING_WEBHOOKS_SECTION)) return null;
  return (
    <section id="outgoing-webhooks">
      <SettingsSectionView slug={OUTGOING_WEBHOOKS_SECTION} />
    </section>
  );
}
