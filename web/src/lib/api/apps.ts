"use client";

import { keepPreviousData, useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { wsRoutes } from "@/lib/routes";
import { traceAction } from "@/lib/telemetry";

import { api, ApiError } from "./client";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
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
  setup: (app: string, scopeId: string) => ["app-setup", app, scopeId] as const,
  connections: (s: CredScope) => ["connections", s.kind, "id" in s ? s.id : ""] as const,
  clients: (s: CredScope) => ["oauth-clients", s.kind, "id" in s ? s.id : ""] as const,
  availability: (org: string) => ["app-availability", org] as const,
  instances: (s: InstanceScope) =>
    ["app-instances", s.kind, "id" in s ? s.id : "", s.kind === "instance" ? `${s.all ?? false}:${s.org ?? ""}` : ""] as const,
};

/**
 * Where self-hosted apps (app instances, D-74) are listed/registered: an organization (its own and the
 * instance-wide ones) or instance-wide. `all` (the admin list, FIX-132) also lists every organization's
 * registrations, optionally of one `org`.
 */
export type InstanceScope = { kind: "organization"; id: string } | { kind: "instance"; all?: boolean; org?: string };

/** Self-hosted apps (D-74): instance-wide and the org's own for an org scope. */
export function useAppInstances(scope: InstanceScope | undefined) {
  return useQuery({
    queryKey: scope ? appKeys.instances(scope) : ["app-instances", "none"],
    queryFn: () =>
      scope!.kind === "organization"
        ? unwrap(api.GET("/v1/orgs/{org}/app-instances", { params: { path: { org: scope!.id } } }))
        : unwrap(
            api.GET("/v1/admin/app-instances", {
              params: { query: { scope: scope!.all ? "all" : undefined, org: scope!.org || undefined } },
            }),
          ),
    enabled: !!scope,
    // Changing the admin list's organization filter keeps the table (and the filter) on screen.
    placeholderData: keepPreviousData,
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

/** Make an organization's registration instance-wide (instance admin, FIX-132). */
export function usePromoteAppInstance() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.POST("/v1/admin/app-instances/{id}/promote", { params: { path: { id } } })),
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

/** The manifest's `grants` flags (P4-GRANTS, D-123, C-410): carried by apps, app details and connection grants. */
export type AppRestrictions = components["schemas"]["AppRestrictions"];

/** Anything that carries `app_restrictions`: an `AppSummary`, an `AppDetail` or a `ConnectionGrant`. */
export interface HasAppRestrictions {
  app_restrictions?: AppRestrictions | null;
}

/**
 * An app that allows only custom grants (`custom_only`): full access is not offered, and the grant step asks for the
 * tools first. A 422 `full_grant_forbidden` (C-411) still teaches the editor the same on a stale page.
 */
export function appIsCustomOnly(app: HasAppRestrictions | null | undefined): boolean {
  return app?.app_restrictions?.custom_only === true;
}

/**
 * The `app_restrictions` of the first sources that carry them, merged: restrictions only tighten, so a flag set by any
 * source (a grant item, the grants response, the app) applies.
 */
export function appRestrictions(...sources: (HasAppRestrictions | null | undefined)[]): AppRestrictions {
  const out: AppRestrictions = { custom_only: false, block_untooled: false };
  for (const s of sources) {
    if (s?.app_restrictions?.custom_only === true) out.custom_only = true;
    if (s?.app_restrictions?.block_untooled === true) out.block_untooled = true;
  }
  return out;
}

/** (C-931) Where a setup page (and its guide) is read: a workspace's view or an organization's. */
export type SetupScope = { workspaceId: string; orgId?: undefined } | { orgId: string; workspaceId?: undefined };

function setupQuery(scope: SetupScope): { workspace_id: string } | { org_id: string } {
  return scope.workspaceId ? { workspace_id: scope.workspaceId } : { org_id: scope.orgId! };
}

export type AppSetup = components["schemas"]["AppSetupStatus"];
export type SetupClientRef = components["schemas"]["ClientRef"];

/** (C-931) The setup status of one app for a workspace or an organization: methods, clients, instances, values, `can`. */
export function useAppSetup(app: string | undefined, scope: SetupScope) {
  return useQuery({
    queryKey: appKeys.setup(app ?? "", scope.workspaceId ?? scope.orgId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/apps/{app}/setup", { params: { path: { app: app! }, query: setupQuery(scope) } })),
    enabled: !!app,
    retry: false,
  });
}

/** `scope`: the guide is filled with the viewer's values (C-940: callback URL, external id, page links). */
export function useAppGuide(app: string | undefined, enabled = true, scope?: SetupScope) {
  return useQuery({
    queryKey: [...appKeys.guide(app ?? ""), scope?.workspaceId ?? scope?.orgId ?? ""] as const,
    queryFn: async (): Promise<string | null> => {
      const { data, response } = await api.GET("/v1/apps/{app}/guide", {
        params: { path: { app: app! }, ...(scope ? { query: setupQuery(scope) } : {}) },
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
      fetchAllPages((cursor) =>
        scope.kind === "workspace"
          ? unwrap(
              api.GET("/v1/workspaces/{ws}/connections", {
                params: { path: { ws: scope.id }, query: { limit: PAGE_LIMIT, cursor } },
              }),
            )
          : unwrap(
              api.GET("/v1/orgs/{org}/connections", {
                params: { path: { org: scope.id }, query: { limit: PAGE_LIMIT, cursor } },
              }),
            ),
      ),
    retry: false,
  });
}

export function usePatchConnection(scope: CredScope & { kind: "workspace" | "organization" }) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string; status?: "active" | "disabled"; label?: string; access?: "read" | "write"; limit?: "read" | "ask" | "act"; scopes?: string[] }) =>
      unwrap(api.PATCH("/v1/connections/{id}", { params: { path: { id } }, body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: appKeys.connections(scope) });
      void qc.invalidateQueries({ queryKey: ["apps"] });
    },
  });
}

export type LimitPreview = components["schemas"]["LimitPreview"];

/** (C-990) Who a lower limit would change: the confirmation before lowering shows it. Asked only when `limit` is given. */
export function useLimitPreview(connectionId: string, limit: "read" | "ask" | "act" | null) {
  return useQuery({
    queryKey: ["connections", connectionId, "limit-preview", limit],
    queryFn: () => unwrap(api.GET("/v1/connections/{id}/limit-preview", { params: { path: { id: connectionId }, query: { limit: limit! } } })),
    enabled: limit !== null,
    retry: false,
    staleTime: 0,
    gcTime: 0,
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

/** A client changed: its list, and every app's setup status and connect plan that counted it (C-931). */
function refreshClients(qc: QueryClient, scope: CredScope): Promise<void> {
  void qc.invalidateQueries({ queryKey: ["app-setup"] });
  void qc.invalidateQueries({ queryKey: ["app-connect-plan"] });
  return qc.invalidateQueries({ queryKey: appKeys.clients(scope) });
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
    onSuccess: () => refreshClients(qc, scope),
  });
}

export function usePatchOAuthClient(scope: CredScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string } & components["schemas"]["PatchOAuthClientRequest"]) =>
      unwrap(api.PATCH("/v1/oauth-clients/{id}", { params: { path: { id } }, body })),
    onSuccess: () => refreshClients(qc, scope),
  });
}

export function useDeleteOAuthClient(scope: CredScope) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const { error, response } = await api.DELETE("/v1/oauth-clients/{id}", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => refreshClients(qc, scope),
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

export type ConnectPlan = components["schemas"]["ConnectPlan"];
export type SetupMethod = components["schemas"]["SetupMethod"];

/**
 * (C-955) What the journey needs to know about an app before it asks anything: which methods work for people here,
 * the suggested one, and who can fix a blocked app. Read when a tile is focused or chosen, never for every app at once.
 */
export function useConnectPlan(app: string | undefined, workspaceId: string | undefined) {
  return useQuery({
    queryKey: ["app-connect-plan", app ?? "", workspaceId ?? ""] as const,
    queryFn: () =>
      unwrap(api.GET("/v1/apps/{app}/connect-plan", { params: { path: { app: app! }, query: { workspace_id: workspaceId! } } })),
    enabled: !!app && !!workspaceId,
    retry: false,
  });
}

export function connectApp(app: string, body: ConnectRequest): Promise<ConnectResult> {
  return traceAction("app_connect", () => unwrap(api.POST("/v1/apps/{app}/connect", { params: { path: { app } }, body })));
}

// ---- OAuth sign-in window and its result channel (D-272, C-842) ----

export const CONNECT_CHANNEL = "aurora-connect";

/** What the OAuth callback page reports on the channel (ids and scope names; nothing secret). */
export type ConnectChannelMessage =
  | {
      type: "connected";
      appId: string;
      connectionId: string;
      missingScopes: string[];
      /** (C-957) The agents the sign-in gave access to; empty when none was asked for or it failed. */
      grantedAgentIds: string[];
      /** (C-957) The code of the access part's failure: the account was made, the access was not. */
      grantFailed: string | null;
    }
  | { type: "failed"; appId: string | null; error: string };

/** Accept only the callback page's two message shapes; anything else (including our own acks) is null. */
export function parseConnectMessage(data: unknown): ConnectChannelMessage | null {
  if (typeof data !== "object" || data === null) return null;
  const d = data as Record<string, unknown>;
  if (d.type === "aurora:connected" && typeof d.app_id === "string" && typeof d.connection_id === "string") {
    const missing = Array.isArray(d.missing_scopes) ? d.missing_scopes.filter((s): s is string => typeof s === "string") : [];
    const granted = Array.isArray(d.granted_agent_ids) ? d.granted_agent_ids.filter((s): s is string => typeof s === "string") : [];
    const failed = d.grant_failed;
    const grantFailed =
      typeof failed === "object" && failed !== null && typeof (failed as { code?: unknown }).code === "string"
        ? (failed as { code: string }).code
        : null;
    return { type: "connected", appId: d.app_id, connectionId: d.connection_id, missingScopes: missing, grantedAgentIds: granted, grantFailed };
  }
  if (d.type === "aurora:connect-failed" && typeof d.error === "string") {
    return { type: "failed", appId: typeof d.app_id === "string" ? d.app_id : null, error: d.error };
  }
  return null;
}

/** Tell the callback page a dialog took its result, so the page closes itself. */
export function ackConnect(ack: { connectionId?: string; error?: string }): void {
  try {
    const ch = new BroadcastChannel(CONNECT_CHANNEL);
    ch.postMessage({
      type: "aurora:connect-ack",
      ...(ack.connectionId ? { connection_id: ack.connectionId } : {}),
      ...(ack.error ? { error: ack.error } : {}),
    });
    ch.close();
  } catch {
    // No BroadcastChannel: the page stays and shows its link.
  }
}

/** Listens for the callback page's result on the channel. Every dashboard tab uses it to refresh its lists. */
export function useConnectChannel(onMessage: (m: ConnectChannelMessage) => void) {
  useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    const ch = new BroadcastChannel(CONNECT_CHANNEL);
    ch.onmessage = (ev: MessageEvent) => {
      const m = parseConnectMessage(ev.data);
      if (m) onMessage(m);
    };
    return () => ch.close();
  }, [onMessage]);
}

/**
 * The OAuth provider's sign-in page is the one thing opened in a separate window. Call this synchronously in the click
 * handler (an `about:blank` window is not blocked by popup blockers), then {@link navigateSignIn} once the redirect URL is known.
 */
export function openSignInWindow(): Window | null {
  return window.open("about:blank", "aurora-sign-in", "popup=yes,width=560,height=760");
}

/** The provider's page must not get a handle on the dashboard window (reverse tabnabbing, WEBC-3). */
export function navigateSignIn(w: Window, url: string): void {
  Reflect.set(w, "opener", null);
  w.location.replace(url);
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

/** The connect deep link (I-29, C-842): the Apps tab with the Connect dialog open, or the Connections tab with Reconnect open. */
export function connectDeepLink(org: string, ws: string, app: string, reconnectId?: string | null): string {
  return wsRoutes(org, ws).connect(app, reconnectId);
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
