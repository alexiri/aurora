"use client";

import { LegacyGate } from "@/components/legacy-redirect";

/**
 * Links already sent (the agent's connection request, I-29) point here. Connecting is a dialog on the Apps tab now
 * (D-272, C-842), so this route only redirects to the deep link (C-960); `?reconnect=<id>` opens the Reconnect dialog.
 */
export default function Page() {
  return <LegacyGate>{null}</LegacyGate>;
}
