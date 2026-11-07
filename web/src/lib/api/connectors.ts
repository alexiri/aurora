"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type ConnectorAdmin = S["ConnectorAdmin"];
export type ConnectorInfo = S["ConnectorInfo"];
export type FormField = S["FormField"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const connectorKeys = {
  admin: ["admin", "connectors"] as const,
  info: ["connectors"] as const,
};

/** Fields of a manifest form section (`{ fields: FormField[] }`), tolerant of absent/odd shapes. */
export function formFields(section: unknown): FormField[] {
  if (typeof section !== "object" || section === null) return [];
  const f = (section as { fields?: unknown }).fields;
  return Array.isArray(f) ? (f as FormField[]) : [];
}

export function dedicatedFields(info: ConnectorInfo): FormField[] {
  const d = (info.presence_setup as { dedicated?: unknown } | null)?.dedicated;
  return formFields(d);
}

/** A capability counts as supported unless explicitly false/absent. */
export function hasCapability(info: ConnectorInfo, name: string): boolean {
  return (info.capabilities as Record<string, unknown> | null)?.[name] === true;
}

export function useConnectors() {
  return useQuery({
    queryKey: connectorKeys.info,
    queryFn: () => unwrap(api.GET("/v1/connectors")),
    retry,
  });
}

export function useAdminConnectors() {
  return useQuery({
    queryKey: connectorKeys.admin,
    queryFn: () => unwrap(api.GET("/v1/admin/connectors")),
    retry,
  });
}

export function useProvisionConnector() {
  const qc = useQueryClient();
  return useMutation({
    // The one-time token lives only in the mutation result; never cached.
    gcTime: 0,
    mutationFn: (body: S["CreateConnectorRequest"]) =>
      unwrap(api.POST("/v1/admin/connectors", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: connectorKeys.admin }),
  });
}

export function useRotateConnectorToken() {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (id: string) =>
      unwrap(api.POST("/v1/admin/connectors/{id}/token", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: connectorKeys.admin }),
  });
}

export function useApproveConnector() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, manifest_digest }: { id: string; manifest_digest: string }) =>
      unwrap(
        api.POST("/v1/admin/connectors/{id}/approve", {
          params: { path: { id } },
          body: { manifest_digest },
        }),
      ),
    // A stale manifest must be refetched, so always settle with a refresh.
    onSettled: () => qc.invalidateQueries({ queryKey: connectorKeys.admin }),
  });
}

export function useDeleteConnector() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/admin/connectors/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: connectorKeys.admin }),
  });
}
