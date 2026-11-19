"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { usePatchOrg } from "@/lib/api/org";
import type { Org } from "@/lib/api/queries";
import type { OrgWebhookSettings } from "@/lib/api/webhook";
import { parseRate } from "@/lib/webhook/logic";

/**
 * The organization's switch and limits for Outgoing webhooks (C-912, C-913), on the Apps tab. Stored and edited like the
 * calendar-invitation switch: owners change it. A limit can only be lower than the instance's, which the server checks
 * (`above_instance_ceiling`); empty uses the instance's.
 */
export function OutgoingWebhooksOrgSettings({ org }: { org: Org }) {
  // Gated: until P5-WEBHOOK-2 lands the server neither returns nor accepts these keys, so nothing renders for an organization
  // whose settings lack them (they appear, with their defaults, once the API supports the app).
  return hasOutgoingWebhookSettings(org) ? <OrgWebhookForm org={org} /> : null;
}

/** True when the organization's settings, as the server returns them, carry the outgoing-webhook keys. */
export function hasOutgoingWebhookSettings(org: Org): boolean {
  return "outgoing_webhooks_enabled" in (org.settings as object);
}

function OrgWebhookForm({ org }: { org: Org }) {
  const patch = usePatchOrg(org.id);
  const s = org.settings;
  const [enabled, setEnabled] = useState(s.outgoing_webhooks_enabled ?? true);
  const [perMinute, setPerMinute] = useState(s.outgoing_webhooks_per_minute_max?.toString() ?? "");
  const [perDay, setPerDay] = useState(s.outgoing_webhooks_per_day_max?.toString() ?? "");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState(false);
  const owner = org.my_role === "owner";

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const pm = parseRate(perMinute);
    const pd = parseRate(perDay);
    const errs: Record<string, string> = {};
    if (pm === "invalid") errs.pm = "Enter a whole number of 1 or more, or leave empty.";
    if (pd === "invalid") errs.pd = "Enter a whole number of 1 or more, or leave empty.";
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;
    const settings: OrgWebhookSettings = {
      outgoing_webhooks_enabled: enabled,
      outgoing_webhooks_per_minute_max: pm as number | null,
      outgoing_webhooks_per_day_max: pd as number | null,
    };
    patch.mutate({ settings }, {
      onSuccess: () => setSaved(true),
      onError: (err) => {
        if (err instanceof ApiError && err.code === "above_instance_ceiling") {
          const msg = "This is above the limit your administrator set for the whole instance.";
          setErrors({ pm: pm !== null && pm !== "invalid" ? msg : "", pd: pd !== null && pd !== "invalid" ? msg : "" });
        }
      },
    });
  }

  const ceilingRefused = patch.error instanceof ApiError && patch.error.code === "above_instance_ceiling";

  return (
    <Section
      id="org-outgoing-webhooks"
      title="Outgoing webhooks"
      description="Lets this organization's agents call webhooks that people have set up for them. With it off, no agent can send and no webhook can be connected."
      card
    >
      <Form onSubmit={submit} noValidate className="max-w-md">
        {!owner ? <Notice role="status">Only organization owners can change these settings.</Notice> : null}
        <fieldset disabled={!owner || patch.isPending} className="space-y-4">
          <CheckboxField id="org-wh-enabled" label="Allow outgoing webhooks in this organization" checked={enabled} onChange={setEnabled} />
          <Field id="org-wh-pm" label="Sends per minute, for each webhook (optional)" hint="Leave empty to use the instance's limit. A limit here can only be lower." error={errors.pm || undefined}>
            <input id="org-wh-pm" className={inputClass} inputMode="numeric" value={perMinute} onChange={(e) => setPerMinute(e.target.value)} />
          </Field>
          <Field id="org-wh-pd" label="Sends per day, for each webhook (optional)" error={errors.pd || undefined}>
            <input id="org-wh-pd" className={inputClass} inputMode="numeric" value={perDay} onChange={(e) => setPerDay(e.target.value)} />
          </Field>
        </fieldset>
        {patch.error && !ceilingRefused ? (
          <Notice tone="danger" role="alert">
            {errorMessage(patch.error)}
          </Notice>
        ) : null}
        {saved ? (
          <Notice tone="accent" role="status">
            Outgoing webhook settings updated.
          </Notice>
        ) : null}
        {owner ? (
          <FormActions>
            <Button type="submit" disabled={patch.isPending}>
              {patch.isPending ? "Updating…" : "Update outgoing webhooks"}
            </Button>
          </FormActions>
        ) : null}
      </Form>
    </Section>
  );
}
