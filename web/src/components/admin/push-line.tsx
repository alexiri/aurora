"use client";

import { Card } from "@/components/ui/card";
import { StatusDot } from "@/components/ui/status-dot";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { useInstance } from "@/lib/api/queries";

/**
 * (P4-011B-4, C-836, `docs/operator/web-push.md`; P5-REFINE-20 setup-4) The "Web Push" line of the admin overview, from
 * `/v1/instance` `features.push`: on or off in words. The setting's name and the command that prints the key mode and the
 * device counts (no admin endpoint exposes them yet) are machinery, so they sit under Technical details.
 */
export function PushLine() {
  const instance = useInstance();
  if (!instance.data) return null;
  const on = instance.data.features.push === true;
  return (
    <Card className="space-y-1 p-3 text-sm sm:p-3" data-testid="push-line">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium text-ink">Web Push</span>
        <StatusDot tone={on ? "accent" : "pending"}>{on ? "On" : "Off"}</StatusDot>
        <span className="min-w-0 break-words text-ink-2">
          {on
            ? "People can turn on notifications for approvals on their devices."
            : "People can't get approval notifications on their devices yet. Ask whoever installed Aurora to turn it on."}
        </span>
      </div>
      <TechnicalDetails>
        {on ? "Run `aurora-api push status` for the key and the device counts." : "Set push.enabled in the Helm values."}
      </TechnicalDetails>
    </Card>
  );
}
