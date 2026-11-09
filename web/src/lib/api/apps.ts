"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AppSummary = components["schemas"]["AppSummary"];
export type AppDetail = components["schemas"]["AppDetail"];
export type ConnectMethod = components["schemas"]["ConnectMethod"];
export type FormField = components["schemas"]["FormField"];
export type Connection = components["schemas"]["Connection"];
export type OAuthClient = components["schemas"]["OAuthClient"];
export type AppAvailability = components["schemas"]["AppAvailability"];
export type AppInstance = components["schemas"]["AppInstance"];
export type CreateAppInstanceRequest = components["schemas"]["CreateAppInstanceRequest"];

/** Scope of a connection / OAuth client list. `instance` is only valid for OAuth clients. */
export type CredScope =
  | { kind: "workspace"; id: string }
  | { kind: "organization"; id: string }
  | { kind: "instance" };

export const appKeys = {
  apps: (ws: string | undefined) => ["apps", ws ?? ""] as const,
  app: (app: string, ws: string | undefined) => ["app", app, ws ?? ""] as const,
  guide: (app: string) => ["app-guide", app] as const,
  connections: (s: CredScope) => ["connections", s.kind, "id" in s ? s.id : ""] as const,
  clients: (s: CredScope) => ["oauth-clients", s.kind, "id" in s ? s.id : ""] as const,
  availability: (org: string) => ["app-availability", org] as const,
  instances: (s: InstanceScope) => ["app-instances", s.kind, "id" in s ? s.id : ""] as const,
};

/** Where app instances are listed/registered: an organization (its own and the instance-wide ones) or instance-wide. */
export type InstanceScope = { kind: "organization"; id: string } | { kind: "instance" };

/** Self-hosted app instances (D-74): instance-wide and the org's own for an org scope. */
export function useAppInstances(scope: InstanceScope | undefined) {
  return useQuery({
    queryKey: scope ? appKeys.instances(scope) : ["app-instances", "none"],
    queryFn: () =>
      scope!.kind === "organization"
        ? unwrap(api.GET("/v1/orgs/{org}/app-instances", { params: { path: { org: scope!.id } } }))
        : unwrap(api.GET("/v1/admin/app-instances")),
    enabled: !!scope,
    retry: false,
  });
}

export function useCreateAppInstance(scope: InstanceScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateAppInstanceRequest) =>
      scope.kind === "organization"
        ? unwrap(api.POST("/v1/orgs/{org}/app-instances", { params: { path: { org: scope.id } }, body }))
        : unwrap(api.POST("/v1/admin/app-instances", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["app-instances"] }),
  });
}

export function usePatchAppInstance() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string } & components["schemas"]["PatchAppInstanceRequest"]) =>
      unwrap(api.PATCH("/v1/app-instances/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["app-instances"] }),
  });
}

export function useDeleteAppInstance() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const { error, response } = await api.DELETE("/v1/app-instances/{id}", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["app-instances"] });
      void qc.invalidateQueries({ queryKey: ["connections"] });
      void qc.invalidateQueries({ queryKey: ["oauth-clients"] });
    },
  });
}

export function useApps(workspaceId: string | undefined) {
  return useQuery({
    queryKey: appKeys.apps(workspaceId),
    queryFn: () =>
      unwrap(api.GET("/v1/apps", { params: { query: workspaceId ? { workspace_id: workspaceId } : {} } })),
    retry: false,
  });
}

export function useApp(app: string | undefined, workspaceId: string | undefined) {
  return useQuery({
    queryKey: appKeys.app(app ?? "", workspaceId),
    queryFn: () =>
      unwrap(
        api.GET("/v1/apps/{app}", {
          params: { path: { app: app! }, query: workspaceId ? { workspace_id: workspaceId } : {} },
        }),
      ),
    enabled: !!app,
    retry: false,
  });
}

/**
 * An app that allows only custom grants (`custom_only`, P4-GRANTS, D-123). The field is not in the generated schema
 * yet; until it is, the grant step also falls back on the 422 `full_grant_forbidden` (C-504).
 */
export function appIsCustomOnly(app: unknown): boolean {
  if (typeof app !== "object" || app === null) return false;
  const a = app as { custom_only?: unknown; grants?: { custom_only?: unknown } | null };
  return a.custom_only === true || a.grants?.custom_only === true;
}

export function useAppGuide(app: string | undefined, enabled = true) {
  return useQuery({
    queryKey: appKeys.guide(app ?? ""),
    queryFn: async (): Promise<string | null> => {
      const { data, response } = await api.GET("/v1/apps/{app}/guide", {
        params: { path: { app: app! } },
        parseAs: "text",
      });
      if (response.status === 404) return null;
      if (!response.ok) throw new ApiError("unknown", "Could not load the setup guide", null, response.status);
      return typeof data === "string" ? data : null;
    },
    enabled: !!app && enabled,
    retry: false,
  });
}

export function useConnections(scope: CredScope & { kind: "workspace" | "organization" }) {
  return useQuery({
    queryKey: appKeys.connections(scope),
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/connections", { params: { path: { ws: scope.id } } }))
        : unwrap(api.GET("/v1/orgs/{org}/connections", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

export function usePatchConnection(scope: CredScope & { kind: "workspace" | "organization" }) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string; status?: "active" | "disabled"; label?: string }) =>
      unwrap(api.PATCH("/v1/connections/{id}", { params: { path: { id } }, body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: appKeys.connections(scope) });
      void qc.invalidateQueries({ queryKey: ["apps"] });
    },
  });
}

export function useDeleteConnection(scope: CredScope & { kind: "workspace" | "organization" }) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const { error, response } = await api.DELETE("/v1/connections/{id}", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: appKeys.connections(scope) });
      void qc.invalidateQueries({ queryKey: ["apps"] });
    },
  });
}

export function useOAuthClients(scope: CredScope) {
  return useQuery({
    queryKey: appKeys.clients(scope),
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/oauth-clients", { params: { path: { ws: scope.id } } }))
        : scope.kind === "organization"
          ? unwrap(api.GET("/v1/orgs/{org}/oauth-clients", { params: { path: { org: scope.id } } }))
          : unwrap(api.GET("/v1/admin/oauth-clients")),
    retry: false,
  });
}

export function useCreateOAuthClient(scope: CredScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["CreateOAuthClientRequest"]) =>
      scope.kind === "workspace"
        ? unwrap(api.POST("/v1/workspaces/{ws}/oauth-clients", { params: { path: { ws: scope.id } }, body }))
        : scope.kind === "organization"
          ? unwrap(api.POST("/v1/orgs/{org}/oauth-clients", { params: { path: { org: scope.id } }, body }))
          : unwrap(api.POST("/v1/admin/oauth-clients", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: appKeys.clients(scope) }),
  });
}

export function usePatchOAuthClient(scope: CredScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string } & components["schemas"]["PatchOAuthClientRequest"]) =>
      unwrap(api.PATCH("/v1/oauth-clients/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: appKeys.clients(scope) }),
  });
}

export function useDeleteOAuthClient(scope: CredScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const { error, response } = await api.DELETE("/v1/oauth-clients/{id}", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: appKeys.clients(scope) }),
  });
}

export function useAppAvailability(orgId: string | undefined) {
  return useQuery({
    queryKey: appKeys.availability(orgId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/app-availability", { params: { path: { org: orgId! } } })),
    enabled: !!orgId,
    retry: false,
  });
}

export function usePutAppAvailability(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AppAvailability) =>
      unwrap(api.PUT("/v1/orgs/{org}/app-availability", { params: { path: { org: orgId } }, body })),
    onSuccess: (data) => {
      qc.setQueryData(appKeys.availability(orgId), data);
      void qc.invalidateQueries({ queryKey: ["apps"] });
    },
  });
}

// ---- connect (C-14) ----

export type ConnectRequest = components["schemas"]["ConnectRequest"];
export type ConnectResult = components["schemas"]["ConnectResponse"];
export type OAuthClientDefinition = components["schemas"]["AppOauthClient"];

export function connectApp(app: string, body: ConnectRequest): Promise<ConnectResult> {
  return unwrap(api.POST("/v1/apps/{app}/connect", { params: { path: { app } }, body }));
}

export const CONNECT_MESSAGE = "aurora:connected";

/** Accept only same-origin `aurora:connected` messages; returns the connection id. */
export function parseConnectedMessage(ev: { origin: string; data: unknown }, origin: string): string | null {
  if (ev.origin !== origin) return null;
  const d = ev.data;
  if (typeof d !== "object" || d === null) return null;
  const { type, connection_id } = d as Record<string, unknown>;
  return type === CONNECT_MESSAGE && typeof connection_id === "string" ? connection_id : null;
}

/** Listens for the popup's `aurora:connected` message. */
export function useConnectedListener(onConnected: (connectionId: string) => void) {
  useEffect(() => {
    const handler = (ev: MessageEvent) => {
      const id = parseConnectedMessage(ev, window.location.origin);
      if (id) onConnected(id);
    };
    window.addEventListener("message", handler);
    return () => window.removeEventListener("message", handler);
  }, [onConnected]);
}

export function openConnectWindow(url: string): Window | null {
  return window.open(url, "aurora-connect", "popup=yes,width=560,height=760");
}

// ---- pure form helpers (unit tested) ----

export type FieldValues = Record<string, string>;

export function isFieldVisible(f: FormField, values: FieldValues): boolean {
  return !f.show_when || (values[f.show_when.field] ?? "") === f.show_when.equals;
}

export function visibleFields(fields: FormField[], values: FieldValues): FormField[] {
  return fields.filter((f) => isFieldVisible(f, values));
}

/** Error messages by field name for visible required fields that are empty or invalid. */
export function validateFields(fields: FormField[], values: FieldValues): Record<string, string> {
  const errs: Record<string, string> = {};
  for (const f of visibleFields(fields, values)) {
    const v = values[f.name] ?? "";
    if (f.required && v.trim() === "") errs[f.name] = `${f.label} is required.`;
    else if (f.type === "json_file" && v !== "" && !parseJsonFile(v).ok) errs[f.name] = `${f.label} is not valid JSON.`;
  }
  return errs;
}

/** Only visible, non-empty values are submitted (hidden fields never leak). */
export function buildConnectFields(fields: FormField[], values: FieldValues): FieldValues {
  const out: FieldValues = {};
  for (const f of visibleFields(fields, values)) {
    const v = values[f.name];
    if (v !== undefined && v !== "") out[f.name] = v;
  }
  return out;
}

/** json_file fields are read client-side; the JSON object text is submitted verbatim after validation. */
export function parseJsonFile(text: string): { ok: true; value: Record<string, unknown> } | { ok: false } {
  try {
    const v: unknown = JSON.parse(text);
    if (typeof v === "object" && v !== null && !Array.isArray(v)) return { ok: true, value: v as Record<string, unknown> };
  } catch {
    // fall through
  }
  return { ok: false };
}

export function defaultValues(fields: FormField[]): FieldValues {
  const out: FieldValues = {};
  for (const f of fields) if (f.type === "select" && f.options?.length && f.required) out[f.name] = f.options[0].value;
  return out;
}

export function connectWindowPath(org: string, ws: string, app: string, reconnectId?: string): string {
  const base = `/o/${org}/w/${ws}/apps/${encodeURIComponent(app)}/connect`;
  return reconnectId ? `${base}?reconnect=${encodeURIComponent(reconnectId)}` : base;
}

/** Toggle an allow-list entry (app, workspace-or-null). */
export function toggleAllow(allow: AppAvailability["allow"], appId: string, workspaceId: string | null): AppAvailability["allow"] {
  const same = (e: AppAvailability["allow"][number]) => e.app_id === appId && (e.workspace_id ?? null) === workspaceId;
  return allow.some(same)
    ? allow.filter((e) => !same(e))
    : [...allow, workspaceId ? { app_id: appId, workspace_id: workspaceId } : { app_id: appId }];
}

/** On PATCH only changed/non-empty fields are sent, so untouched secrets keep their stored value. */
export function buildClientPatchFields(edits: FieldValues): FieldValues {
  const out: FieldValues = {};
  for (const [k, v] of Object.entries(edits)) if (v !== "") out[k] = v;
  return out;
}
