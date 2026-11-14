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

export type ConnectorType = S["ConnectorType"];
export type ConnectorTypeList = S["ConnectorTypeList"];
export type ConnectorSecretInfo = S["ConnectorSecretInfo"];
export type PatchConnectorRequest = S["PatchConnectorRequest"];
export type CreateConnectorRequest = S["CreateConnectorRequest"];

/**
 * An `instance_setup` field (C-450): the form-field schema plus `url`, `number` (`min`/`max`),
 * `generated_secret` and a static `default`. The generated type lists the base schema only.
 */
export type SetupField = FormField & {
  help?: string | null;
  default?: unknown;
  min?: number | null;
  max?: number | null;
  /** (C-715) A hint: the dashboard collapses the field under "Advanced". */
  advanced?: boolean;
  /** (C-710/C-715) `"from_validate"`: the options come from the probe (`POST …:probe`). */
  choices?: string;
};

/**
 * (D-219) A connector type whose instance settings are all optional or generated can be added with
 * one button: nothing has to be typed.
 */
export function isOneClick(type: Pick<ConnectorType, "instance_setup">): boolean {
  return setupFields(type.instance_setup).every((f) => !f.required || f.type === "generated_secret");
}

export type ConnectorAvailability = S["ConnectorAvailability"];
export type ConnectorSetupState = S["ConnectorSetupState"];

export const connectorKeys = {
  admin: ["admin", "connectors"] as const,
  info: ["connectors"] as const,
  types: ["admin", "connector-types"] as const,
  availability: ["connector-availability"] as const,
};

/** Per announced connector type: how far from usable, and who to ask (C-712). */
export function useConnectorAvailability() {
  return useQuery({
    queryKey: connectorKeys.availability,
    queryFn: () => unwrap(api.GET("/v1/connector-availability")),
    retry,
  });
}

/** Does this integration form need the probe (C-710): a field takes its choices from it. */
export function needsProbe(fields: FormField[]): boolean {
  return (fields as SetupField[]).some((f) => f.choices === "from_validate");
}

export function setupFields(section: unknown): SetupField[] {
  return formFields(section) as SetupField[];
}

/** The connector's id is the type id; the icon route is shared with `ConnectorInfo.icon_url`. */
export const connectorIconUrl = (id: string) => `/v1/connectors/${encodeURIComponent(id)}/icon`;

export function useConnectorTypes() {
  return useQuery({
    queryKey: connectorKeys.types,
    queryFn: () => unwrap(api.GET("/v1/admin/connector-types")),
    retry,
  });
}

/** Add a managed connector (C-451). The response has no token. */
export function useCreateManagedConnector() {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: CreateConnectorRequest) =>
      unwrap(api.POST("/v1/admin/connectors", { body: { ...body, managed: true } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: connectorKeys.admin });
      void qc.invalidateQueries({ queryKey: connectorKeys.types });
      void qc.invalidateQueries({ queryKey: connectorKeys.availability });
      void qc.invalidateQueries({ queryKey: connectorKeys.info });
    },
    // A stale digest means the type changed: reload it so Review shows the new manifest.
    onError: () => qc.invalidateQueries({ queryKey: connectorKeys.types }),
  });
}

/** Edit a connector (C-453): returns the bare connector. Secrets in the body are write-only. */
export function usePatchConnector() {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: ({ id, body }: { id: string; body: PatchConnectorRequest }) =>
      unwrap(api.PATCH("/v1/admin/connectors/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: connectorKeys.admin }),
  });
}

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

export type HostToken = S["HostTokenInfo"];

export const hostTokenKeys = ["admin", "connector-host-tokens"] as const;

/** Connector-host tokens (C-690): the shared bootstrap token and every per-host token. */
export function useHostTokens() {
  return useQuery({
    queryKey: hostTokenKeys,
    queryFn: () => unwrap(api.GET("/v1/admin/connector-host-tokens")),
    retry,
  });
}

/** Issue a token bound to connector types (C-691); the value is only in the mutation result. */
export function useIssueHostToken() {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: S["IssueHostTokenRequest"]) =>
      unwrap(api.POST("/v1/admin/connector-host-tokens", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: hostTokenKeys }),
  });
}

export function useRevokeHostToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/admin/connector-host-tokens/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: hostTokenKeys }),
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
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: connectorKeys.admin });
      void qc.invalidateQueries({ queryKey: connectorKeys.types });
    },
  });
}
