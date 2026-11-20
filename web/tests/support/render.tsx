import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactNode } from "react";

import { onboardingKeys, type MeOnboarding } from "@/lib/api/onboarding";

/**
 * Puts the person's "Show more controls" switch (C-1056) in the client's cache, as `GET /v1/me/onboarding` would, and keeps
 * it from being refetched: the Expert controls of a page then render in the test (`more: false` is the default).
 */
export function seedMoreControls(qc: QueryClient, more = true) {
  const data: MeOnboarding = { welcome_seen: true, guide_dismissed: [], tour_completed_at: null, notices_seen: [], more_controls: more };
  qc.setQueryDefaults(onboardingKeys.me, { staleTime: Infinity });
  qc.setQueryData(onboardingKeys.me, data);
}

/**
 * Renders `ui` inside a fresh QueryClient that never retries (tests assert the first failure). "Show more controls" is on
 * by default (D-406), so a test of an Expert control sees it; pass `moreControls: false` to test Use, or `undefined` to
 * leave the switch unanswered.
 */
export function wrap(ui: ReactNode, opts: { moreControls?: boolean | undefined } = { moreControls: true }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  if (opts.moreControls !== undefined) seedMoreControls(qc, opts.moreControls);
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
