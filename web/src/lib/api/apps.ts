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
};

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

export interface ConnectRequest {
  scope: "organization" | "workspace";
  workspace_id?: string;
  method_id: string;
  fields?: Record<string, string>;
  oauth_client_id?: string;
  access_level?: string;
  reconnect_connection_id?: string;
}
export type ConnectResult =
  | { kind: "redirect"; url: string }
  | { kind: "connected"; connection: Connection; missing_scopes: string[] };

// TODO(FE-030): replace with the generated `/v1/apps/{app}/connect` operation once BE-043 lands in schema.d.ts.
export async function connectApp(app: string, body: ConnectRequest): Promise<ConnectResult> {
  const res = await fetch(`/v1/apps/${encodeURIComponent(app)}/connect`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const json: unknown = await res.json().catch(() => null);
  if (!res.ok) throw ApiError.fromBody(json, res.status);
  return json as ConnectResult;
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

/** Secret fields of an OAuth client are write-only: names that look secret render as password inputs and are never prefilled. */
export function isSecretFieldName(name: string): boolean {
  return /secret|token|password|private/i.test(name);
}

/** On PATCH only changed/non-empty fields are sent, so untouched secrets keep their stored value. */
export function buildClientPatchFields(edits: FieldValues): FieldValues {
  const out: FieldValues = {};
  for (const [k, v] of Object.entries(edits)) if (v !== "") out[k] = v;
  return out;
}
