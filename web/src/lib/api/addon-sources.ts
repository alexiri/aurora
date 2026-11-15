"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AddonSource = components["schemas"]["AddonSource"];
export type AddonEntry = components["schemas"]["AddonEntry"];
export type AddonVersion = components["schemas"]["AddonVersion"];
export type AddonSourcePreview = components["schemas"]["AddonSourcePreview"];
export type AddonSourceCheckResult = components["schemas"]["AddonSourceCheckResult"];

/** The schedule values of the owner's enum, in display order (`never` = manual checks only). */
export const INTERVALS = ["6h", "12h", "1d", "1w", "2w", "1mo", "never"] as const;
export type Interval = (typeof INTERVALS)[number];

export const INTERVAL_LABELS: Record<Interval, string> = {
  "6h": "Every 6 hours",
  "12h": "Every 12 hours",
  "1d": "Every day",
  "1w": "Every week",
  "2w": "Every 2 weeks",
  "1mo": "Every month",
  never: "Never (manual checks only)",
};

export function intervalLabel(v: string): string {
  return INTERVAL_LABELS[v as Interval] ?? v;
}

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const addonKeys = {
  all: ["admin", "addon-sources"] as const,
};

/** Registering, checking and editing change sources, their packages and the plugin badges. */
function useInvalidate() {
  const qc = useQueryClient();
  return () => Promise.all([qc.invalidateQueries({ queryKey: ["admin", "addon-sources"] }), qc.invalidateQueries({ queryKey: ["admin", "plugins"] })]);
}

export function useAddonSources() {
  return useQuery({
    queryKey: addonKeys.all,
    queryFn: () => unwrap(api.GET("/v1/admin/addon-sources")),
    retry,
  });
}

export function usePreviewAddonSource() {
  return useMutation({
    mutationFn: (url: string) => unwrap(api.POST("/v1/admin/addon-sources/preview", { body: { url } })),
  });
}

export function useAddAddonSource() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (body: { url: string; update_interval?: Interval }) =>
      unwrap(api.POST("/v1/admin/addon-sources", { body })),
    onSuccess: invalidate,
  });
}

export function useUpdateAddonSource() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (v: { id: string; update_interval: Interval }) =>
      unwrap(
        api.PATCH("/v1/admin/addon-sources/{id}", {
          params: { path: { id: v.id } },
          body: { update_interval: v.update_interval },
        }),
      ),
    onSuccess: invalidate,
  });
}

export function useRemoveAddonSource() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/admin/addon-sources/{id}", { params: { path: { id } } })),
    onSuccess: invalidate,
  });
}

export function useCheckAddonSource() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.POST("/v1/admin/addon-sources/{id}/check", { params: { path: { id } } })),
    onSettled: invalidate,
  });
}

export function useCheckAllAddonSources() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: () => unwrap(api.POST("/v1/admin/addon-sources/check-all")),
    onSettled: invalidate,
  });
}

export function useConfirmAddonKey() {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (v: { id: string; new_key_fingerprint: string }) =>
      unwrap(
        api.POST("/v1/admin/addon-sources/{id}/confirm-key", {
          params: { path: { id: v.id } },
          body: { new_key_fingerprint: v.new_key_fingerprint },
        }),
      ),
    onSuccess: invalidate,
  });
}

/** One sentence per result of a check, for a toast or a notice. */
export function checkSummary(r: AddonSourceCheckResult, name: string): string {
  if (r.status === "failed") return `${name}: check failed${r.check_failed_reason ? ` — ${r.check_failed_reason}` : ""}`;
  if (r.status === "updated") {
    const n = r.ingested.length;
    return `${name}: ${n} new ${n === 1 ? "version" : "versions"} ready for review`;
  }
  return `${name}: up to date`;
}
