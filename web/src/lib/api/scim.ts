"use client";

// SCIM directory sync (P4-GRP-2, docs/04 §SCIM, docs/05 §SCIM management): the management
// routes of `/v1/orgs/{org}/scim*` are ordinary owner `/v1` routes and part of `openapi.json`
// (C-471/C-472, FIX-081), so the types below are the generated ones. The `/scim/v2` plane
// itself stays outside the document. The calls keep the same envelope conventions
// (`{error:{code,message,details}}`, cookie session).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import type { components } from "./schema";

type Schemas = components["schemas"];

/** `available` is the instance switch (`[scim] enabled`): false = the whole plane is 404. */
export type ScimSettings = Schemas["ScimSettings"];
export type ScimTokenEntry = Schemas["ScimTokenInfo"];
export type ScimStatus = Schemas["ScimStatus"];
/** The token is shown exactly once, at creation; only the prefix is stored. */
export type CreatedScimToken = Schemas["CreatedScimToken"];

const jsonHeaders = { "content-type": "application/json" };

/** One JSON call against the SCIM management surface, mapped to `ApiError`. */
async function call<T>(url: string, method: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? undefined : jsonHeaders,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    let parsed: unknown = null;
    try {
      parsed = await res.json();
    } catch {
      // an empty body (or HTML error page) falls back to the generic message
    }
    throw ApiError.fromBody(parsed, res.status);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const scimKeys = {
  settings: (org: string) => ["orgs", org, "scim"] as const,
  tokens: (org: string) => ["orgs", org, "scim", "tokens"] as const,
  status: (org: string) => ["orgs", org, "scim", "status"] as const,
};

export function useScimSettings(orgId: string | undefined) {
  return useQuery({
    queryKey: scimKeys.settings(orgId ?? ""),
    queryFn: () => call<ScimSettings>(`/v1/orgs/${orgId}/scim`, "GET"),
    enabled: !!orgId,
    retry,
  });
}

export function useScimTokens(orgId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: scimKeys.tokens(orgId ?? ""),
    queryFn: () => call<{ tokens: ScimTokenEntry[] }>(`/v1/orgs/${orgId}/scim/tokens`, "GET"),
    enabled: !!orgId && enabled,
    retry,
  });
}

export function useScimStatus(orgId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: scimKeys.status(orgId ?? ""),
    queryFn: () => call<ScimStatus>(`/v1/orgs/${orgId}/scim/status`, "GET"),
    enabled: !!orgId && enabled,
    retry,
  });
}

export function usePutScimSettings(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: {
      enabled?: boolean;
      email_domains?: string[];
      exclusive?: boolean;
    }) => call<ScimSettings>(`/v1/orgs/${orgId}/scim`, "PUT", body),
    onSuccess: (s) => {
      qc.setQueryData(scimKeys.settings(orgId), s);
      void qc.invalidateQueries({ queryKey: scimKeys.status(orgId) });
    },
  });
}

export function useCreateScimToken(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; expires_in_days?: number }) =>
      call<CreatedScimToken>(`/v1/orgs/${orgId}/scim/tokens`, "POST", body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: scimKeys.tokens(orgId) }),
  });
}

export function useRevokeScimToken(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      call<void>(`/v1/scim/tokens/${id}`, "DELETE"),
    onSuccess: () => void qc.invalidateQueries({ queryKey: scimKeys.tokens(orgId) }),
  });
}

/** Human text of the last sync activity (`05 §SCIM management`). */
export function lastActivityText(s: ScimStatus | undefined): string {
  if (!s) return "";
  const at = s.last_request_at ? new Date(s.last_request_at) : null;
  if (!at || Number.isNaN(at.getTime())) return "No requests yet.";
  const diff = Date.now() - at.getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "Last request just now.";
  if (mins < 60) return `Last request ${mins} minute${mins === 1 ? "" : "s"} ago.`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `Last request ${hours} hour${hours === 1 ? "" : "s"} ago.`;
  const days = Math.floor(hours / 24);
  return `Last request ${days} day${days === 1 ? "" : "s"} ago.`;
}
