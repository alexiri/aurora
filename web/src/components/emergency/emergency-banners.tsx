"use client";

// (FIX-236, C-1165) The two emergency banners, above the page content on every workspace page:
// - the reader's own banner while `emergency_reader` is true, with "End now" (POST …/end, then the
//   workspace answer's `emergency_reader` turns false and the mode leaves at once);
// - the workspace people's banner while `audience.emergency` is set (active, or ended within the
//   last 7 days), dismissible per person through local storage inside try/catch.
// A caller who holds the grant sees the first one only: they have been told already, by opening it.

import { useState } from "react";
import { useParams } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useEndEmergencyAccess } from "@/lib/api/emergency";
import { useResolve, useWorkspace } from "@/lib/api/queries";
import { dismissEmergency, emergencyDismissed, emergencyPeopleBannerText, emergencyReaderBannerText, type WorkspaceEmergency } from "@/lib/emergency";

/** The reader's banner: what the mode is, until when, and the one action that ends it early. */
function ReaderBanner({ expiresAt, emergencyId }: { expiresAt: string; emergencyId: string }) {
  const end = useEndEmergencyAccess();
  return (
    <Notice tone="pending" role="note" className="flex flex-wrap items-center gap-x-3 gap-y-2" data-testid="emergency-reader-banner">
      <p className="min-w-0 flex-1">{emergencyReaderBannerText(expiresAt)}</p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={end.isPending}
        onClick={() => end.mutate(emergencyId)}
        data-testid="emergency-end-now"
      >
        {end.isPending ? "Ending…" : "End now"}
      </Button>
    </Notice>
  );
}

/** The people's banner: who opened it, why and until when; "Dismiss" remembers, per person and per grant. */
function PeopleBanner({ emergency }: { emergency: WorkspaceEmergency }) {
  const [dismissed, setDismissed] = useState(() => emergencyDismissed(emergency.id));
  if (dismissed) return null;
  return (
    <Notice tone="pending" role="note" className="flex flex-wrap items-center gap-x-3 gap-y-2" data-testid="emergency-people-banner">
      <p className="min-w-0 flex-1">{emergencyPeopleBannerText(emergency)}</p>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={() => {
          dismissEmergency(emergency.id);
          setDismissed(true);
        }}
        data-testid="emergency-banner-dismiss"
      >
        Dismiss
      </Button>
    </Notice>
  );
}

/** One of the two banners, or nothing: no workspace in the address means nothing to say. */
export function EmergencyBanners() {
  const { org, ws } = useParams<{ org?: string; ws?: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  if (!ws) return null;
  const w = workspace.data;
  if (!w) return null;
  const emergency = w.audience?.emergency;
  if (w.emergency_reader) {
    // The active grant's id is the one in `audience.emergency` (C-1165: the newest active-or-recent row).
    return emergency ? <ReaderBanner expiresAt={emergency.expires_at} emergencyId={emergency.id} /> : null;
  }
  return emergency ? <PeopleBanner emergency={emergency} /> : null;
}
