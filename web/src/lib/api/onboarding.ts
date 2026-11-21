"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type MeOnboarding = S["MeOnboarding"];
export type PatchOnboarding = S["PatchOnboarding"];
export type SetupStatus = S["SetupStatus"];
export type SetupStep = S["SetupStep"];
export type SetupCheck = S["SetupCheck"];
/** (C-1063) The "Try it" card from the server: offered only once an agent can answer; done after a finished turn. */
export type SetupTryIt = S["SetupTryIt"];

export const onboardingKeys = {
  me: ["me", "onboarding"] as const,
  setup: ["admin", "setup"] as const,
};

/** `GET /v1/me/onboarding` (C-530): Welcome seen, dismissed guides, tour. */
export function useMeOnboarding(enabled = true) {
  return useQuery({
    queryKey: onboardingKeys.me,
    enabled,
    queryFn: () => unwrap(api.GET("/v1/me/onboarding")),
    retry: false,
  });
}

/** `PATCH /v1/me/onboarding`: only the fields sent change; the answer replaces the cached state. */
export function usePatchOnboarding() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchOnboarding) => unwrap(api.PATCH("/v1/me/onboarding", { body })),
    onSuccess: (data) => qc.setQueryData(onboardingKeys.me, data),
  });
}

/** `GET /v1/admin/setup` (C-533): the wizard's live status. Refetched when the page regains focus. */
export function useAdminSetup(enabled = true) {
  return useQuery({
    queryKey: onboardingKeys.setup,
    enabled,
    queryFn: () => unwrap(api.GET("/v1/admin/setup")),
    retry: false,
    refetchOnWindowFocus: true,
    // (P5-REFINE-20) An AI key exists but its agent cannot answer yet (the key is still being checked): read the status again
    // every few seconds, so Try it appears without a reload.
    refetchInterval: (q) => {
      const d = q.state.data;
      const keyOk = d?.steps.find((s) => s.id === "llm")?.checks.find((c) => c.id === "org_key")?.status === "ok";
      const t = d?.try_it;
      return keyOk && t && !t.available && !t.done ? 5_000 : false;
    },
  });
}
