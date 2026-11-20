"use client";

import { LegacyGate } from "@/components/legacy-redirect";
import { Loading } from "@/components/states";

/**
 * (P5-REFINE-18 #1) The old Agents topic is gone: Defaults, BYO and Skills are tabs of What agents may do, Computers
 * is a tab of Health & runners. `LegacyGate` replaces this address by the tab that took over (an address with no
 * known tab lands on Defaults).
 */
export default function Page() {
  return (
    <LegacyGate>
      <Loading />
    </LegacyGate>
  );
}
