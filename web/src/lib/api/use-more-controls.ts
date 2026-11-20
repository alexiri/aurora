"use client";

import { QueryClientContext, useMutation, useQueryClient } from "@tanstack/react-query";
import { useContext, useState } from "react";

import { api } from "./client";
import { errorMessage } from "./errors";
import { onboardingKeys, useMeOnboarding, type MeOnboarding } from "./onboarding";
import { unwrap } from "./unwrap";

export interface MoreControls {
  /** The person's "Show more controls" switch (C-1056). `false` while the first answer is pending: no flash of Expert controls. */
  more: boolean;
  setMore: (on: boolean) => void;
  pending: boolean;
  /** The standard error sentence of the last failed change (the value is reverted), `null` otherwise. */
  error: string | null;
}

/**
 * (P5-REFINE-3, D-406) The per-person switch, stored next to `welcome_seen` so it follows the person across browsers.
 * Optimistic: the cached answer changes at once and is put back when the save fails.
 */
export function useMoreControls(): MoreControls {
  // Without a QueryClient (a control rendered alone in a unit test) the switch is off and inert.
  const hasClient = useContext(QueryClientContext) !== undefined;
  // eslint-disable-next-line react-hooks/rules-of-hooks
  return hasClient ? useLiveMoreControls() : INERT;
}

const INERT: MoreControls = { more: false, setMore: () => undefined, pending: false, error: null };

function useLiveMoreControls(): MoreControls {
  const qc = useQueryClient();
  const onboarding = useMeOnboarding();
  const [error, setError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (more: boolean) => unwrap(api.PATCH("/v1/me/onboarding", { body: { more_controls: more } })),
    onMutate: async (more) => {
      setError(null);
      await qc.cancelQueries({ queryKey: onboardingKeys.me });
      const before = qc.getQueryData<MeOnboarding>(onboardingKeys.me);
      if (before) qc.setQueryData<MeOnboarding>(onboardingKeys.me, { ...before, more_controls: more });
      return { before };
    },
    onError: (err, _more, ctx) => {
      if (ctx?.before) qc.setQueryData(onboardingKeys.me, ctx.before);
      setError(errorMessage(err));
    },
    onSuccess: (data) => qc.setQueryData(onboardingKeys.me, data),
  });
  return {
    more: onboarding.data?.more_controls === true,
    setMore: (on) => save.mutate(on),
    pending: save.isPending,
    error,
  };
}
