"use client";

import { LegacyGate } from "@/components/legacy-redirect";
import { Loading } from "@/components/states";

/**
 * (FIX-245) The address the first template attention items and notices carried: the review of one
 * template version lives under What agents may do › Agent templates, so `LegacyGate` replaces this
 * address by the review page instead of a 404.
 */
export default function Page() {
  return (
    <LegacyGate>
      <Loading />
    </LegacyGate>
  );
}
