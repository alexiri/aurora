"use client";

import { LegacyGate } from "@/components/legacy-redirect";

/** An old address (C-960): the tab decides which topic page took over, so the redirect is made in the browser. */
export default function Page() {
  return <LegacyGate>{null}</LegacyGate>;
}
