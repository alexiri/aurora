"use client";

// P5-WEBHOOK-4 (C-909, C-912, C-913): the Outgoing webhook app's types and calls.
//
// P5-WEBHOOK-2 (the API) is not merged when this file is written, so `schema.d.ts` has none of these routes. The types are
// written here from the contracts, in one module, and the calls go through the traced client for same-origin `/v1`
// routes. When -2 lands and `pnpm gen:api` has produced them, this module's types are replaced by the generated ones
// (`components["schemas"]["WebhookConfig"]`, …) and the calls by `api.GET/PUT/POST`; nothing else imports the wire.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, tracedFetch } from "./client";
import type { ConnectRequest } from "./apps";
import type { Connection } from "./apps";

export type SlotType = "string" | "integer" | "number" | "boolean" | "string_list" | "enum" | "json";
export type ContentType = "application/json" | "application/x-www-form-urlencoded" | "text/plain";

export interface Slot {
  name: string;
  type: SlotType;
  required: boolean;
  description: string;
  max_length?: number;
  max_items?: number;
  enum?: string[];
  schema?: Record<string, unknown>;
  example?: unknown;
}

export interface WebhookHeader {
  name: string;
  value: string;
}

export interface WebhookConfig {
  version: 1;
  method: "POST" | "PUT";
  content_type: ContentType;
  template: string;
  slots: Slot[];
  headers: WebhookHeader[];
  description: string;
  signing: boolean;
  rate: { per_minute: number | null; per_day: number | null };
  preset: string | null;
}

/** A preset is data in the package; it holds no secret value, only the names the person must enter. */
export interface WebhookPreset {
  id: string;
  name: string;
  suggest_for_hosts?: string[];
  secret_value_names: string[];
  config: WebhookConfig;
}

export type SigningRequest = { mode: "generate" } | { mode: "provided"; value: string };
export type SigningEdit = { mode: "keep" } | { mode: "off" } | SigningRequest;

export type RateSource = "instance" | "org" | "connection";
export interface EffectiveRate {
  per_minute: number;
  per_day: number;
  source: RateSource;
}

/** `GET /v1/connections/{id}/webhook`: never a secret, never the URL. */
export interface WebhookView extends WebhookConfig {
  host: string;
  secret_value_names: string[];
  signing: boolean;
  effective_rate: EffectiveRate;
}

export interface WebhookPreview {
  method: string;
  content_type: string;
  headers: string[];
  body: string;
}

export interface WebhookTestResult {
  status: number;
  content_type: string | null;
  body: string;
  truncated: boolean;
  delivery_id: string;
}

export type WebhookConnectResult = { kind: "connected"; connection: Connection; signing_secret?: string; grants?: { agent_id: string }[]; missing_scopes?: string[] };

export interface WebhookConnectBody extends WebhookConfig {
  secret_values: Record<string, string>;
  signing_secret?: SigningRequest;
}

export interface WebhookPutBody extends WebhookConfig {
  secret_values: Record<string, string | null>;
  signing_secret: SigningEdit;
}

async function call<T>(template: string, path: string, method: "GET" | "POST" | "PUT", body?: unknown): Promise<T> {
  const res = await tracedFetch(template, path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? { accept: "application/json" } : { accept: "application/json", "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json: unknown = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = null;
  }
  if (!res.ok) throw ApiError.fromBody(json ?? undefined, res.status);
  return json as T;
}

export const webhookKeys = {
  presets: (app: string) => ["webhook", "presets", app] as const,
  config: (id: string) => ["webhook", "config", id] as const,
};

/** The presets of the installed package version (C-909). They never change within a session. */
export function useWebhookPresets(app = "webhook") {
  return useQuery({
    queryKey: webhookKeys.presets(app),
    queryFn: () => call<WebhookPreset[]>("/v1/apps/{app}/webhook-presets", `/v1/apps/${encodeURIComponent(app)}/webhook-presets`, "GET"),
    staleTime: Infinity,
    retry: false,
  });
}

/** The stored configuration of a connection (Edit, the card's limit line). */
export function useWebhookView(connectionId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: webhookKeys.config(connectionId ?? ""),
    enabled: enabled && !!connectionId,
    queryFn: () =>
      call<WebhookView>("/v1/connections/{id}/webhook", `/v1/connections/${encodeURIComponent(connectionId!)}/webhook`, "GET"),
    retry: false,
    // An open editor is never replaced by a background refetch.
    refetchOnWindowFocus: false,
    // A dialog always starts from what the server holds now, whatever the card cached.
    refetchOnMount: "always",
  });
}

/** Render a configuration with sample values; no network (C-909). Not cached. */
export function previewWebhook(config: WebhookConfig & { secret_value_names: string[] }, values: Record<string, unknown>) {
  return call<WebhookPreview>("/v1/apps/webhook/webhook-preview", "/v1/apps/webhook/webhook-preview", "POST", { config, values });
}

/** Connect (C-909). The body is `ConnectRequest` plus `webhook`; the URL travels in `fields.url` and is never kept. */
export function connectWebhook(body: ConnectRequest & { webhook: WebhookConnectBody }) {
  return call<WebhookConnectResult>("/v1/apps/{app}/connect", "/v1/apps/webhook/connect", "POST", body);
}

/** Replace the configuration (Edit). Secrets: a value sets, `null` deletes, an absent name is kept. */
export function useWebhookPut(connectionId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: WebhookPutBody) =>
      call<WebhookView & { signing_secret?: string }>("/v1/connections/{id}/webhook", `/v1/connections/${encodeURIComponent(connectionId)}/webhook`, "PUT", body),
    onSuccess: (view) => {
      // The one-time signing secret never enters the cache.
      const { signing_secret: _shown, ...rest } = view;
      void _shown;
      qc.setQueryData(webhookKeys.config(connectionId), rest);
      void qc.invalidateQueries({ queryKey: ["accounts"] });
    },
  });
}

/** Send test (C-909): really calls the webhook, or with `dry_run` shows the stored configuration's rendering. */
export function useWebhookTest(connectionId: string) {
  return useMutation({
    mutationFn: (v: { values: Record<string, unknown>; dry_run: boolean }) =>
      call<WebhookTestResult | WebhookPreview>(
        "/v1/connections/{id}/webhook/test",
        `/v1/connections/${encodeURIComponent(connectionId)}/webhook/test`,
        "POST",
        v.dry_run ? { values: v.values, dry_run: true } : { values: v.values },
      ),
  });
}

/** A test answer that was really sent has a status; a dry run has a method. */
export const isSent = (r: WebhookTestResult | WebhookPreview): r is WebhookTestResult => "status" in r;

// ---- The organization's switch and limits (C-912) ----

/** The organization's outgoing-webhook settings, as `PATCH /v1/orgs/{org}` keys them (as `calendar_invitations_enabled`). */
export interface OrgWebhookSettings {
  outgoing_webhooks_enabled?: boolean;
  outgoing_webhooks_per_minute_max?: number | null;
  outgoing_webhooks_per_day_max?: number | null;
}
