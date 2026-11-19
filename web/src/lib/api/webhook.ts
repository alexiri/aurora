"use client";

// P5-WEBHOOK-4 (C-909, C-912, C-913): the Outgoing webhook app's types and calls.
//
// The types are the generated ones (`schema.d.ts`, from P5-WEBHOOK-2's OpenAPI); this module only names them. The calls go
// through the traced client for same-origin `/v1` routes.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, tracedFetch } from "./client";
import type { ConnectRequest } from "./apps";
import type { components } from "./schema";

type S = components["schemas"];

export type SlotType = S["SlotType"];
export type ContentType = S["ContentType"];
export type Slot = S["Slot"];
export type WebhookHeader = S["HeaderTemplate"];

/**
 * The configuration the editor works on: the generated `WebhookConfig` with the server's defaults filled in (the wire
 * leaves `slots`, `headers`, `description`, `signing`, `rate` and `preset` optional), so it is assignable to every wire body.
 */
export type WebhookConfig = Omit<S["WebhookConfig"], "slots" | "headers" | "description" | "signing" | "rate" | "preset"> & {
  slots: Slot[];
  headers: WebhookHeader[];
  description: string;
  signing: boolean;
  rate: Required<S["Rate"]>;
  preset: string | null;
};

/** A preset is data in the package; it holds no secret value, only the names the person must enter. */
export type WebhookPreset = Omit<S["WebhookPreset"], "config" | "secret_value_names"> & {
  config: WebhookConfig;
  secret_value_names: string[];
};

/** How the signing secret is set: connect is `generate` | `provided`; Edit adds `keep` and `off` (`mode` is a string on the wire). */
export type SigningRequest = { mode: "generate" } | { mode: "provided"; value: string };
export type SigningEdit = { mode: "keep" } | { mode: "off" } | SigningRequest;

export type RateSource = "instance" | "org" | "connection";
export type EffectiveRate = Omit<S["EffectiveRateDto"], "source"> & { source: RateSource };

/** `GET /v1/connections/{id}/webhook`: never a secret, never the URL. */
export type WebhookView = WebhookConfig & {
  host: string;
  secret_value_names: string[];
  effective_rate: EffectiveRate;
};
export type WebhookPreview = S["PreviewResponse"];
/** The answer of a real send (the generated `TestResponse` with its sent fields present). */
export type WebhookTestResult = {
  status: number;
  content_type: string | null;
  body: string;
  truncated: boolean;
  delivery_id: string;
};

export type WebhookConnectResult = Extract<S["ConnectResponse"], { kind: "connected" }>;

export type WebhookConnectBody = S["WebhookConnectBody"];
export type WebhookPutBody = S["PutWebhookRequest"];

/** Fill the wire's optional parts with the server's defaults. */
export function normalizeConfig<T extends S["WebhookConfig"]>(c: T): T & WebhookConfig {
  return {
    ...c,
    slots: c.slots ?? [],
    headers: c.headers ?? [],
    description: c.description ?? "",
    signing: c.signing ?? false,
    rate: { per_minute: c.rate?.per_minute ?? null, per_day: c.rate?.per_day ?? null },
    preset: c.preset ?? null,
  };
}

function normalizeView(v: S["WebhookView"]): WebhookView {
  return { ...normalizeConfig(v), effective_rate: v.effective_rate as EffectiveRate };
}

function normalizePreset(p: S["WebhookPreset"]): WebhookPreset {
  return { ...p, config: normalizeConfig(p.config), secret_value_names: p.secret_value_names ?? [] };
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
    queryFn: async () =>
      (await call<S["WebhookPreset"][]>("/v1/apps/{app}/webhook-presets", `/v1/apps/${encodeURIComponent(app)}/webhook-presets`, "GET")).map(normalizePreset),
    staleTime: Infinity,
    retry: false,
  });
}

/** The stored configuration of a connection (Edit, the card's limit line). */
export function useWebhookView(connectionId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: webhookKeys.config(connectionId ?? ""),
    enabled: enabled && !!connectionId,
    queryFn: async () =>
      normalizeView(await call<S["WebhookView"]>("/v1/connections/{id}/webhook", `/v1/connections/${encodeURIComponent(connectionId!)}/webhook`, "GET")),
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
export async function connectWebhook(body: ConnectRequest & { webhook: WebhookConnectBody }): Promise<WebhookConnectResult> {
  const res = await call<S["ConnectResponse"]>("/v1/apps/{app}/connect", "/v1/apps/webhook/connect", "POST", body);
  // A send-mode webhook is never an OAuth redirect.
  if (res.kind !== "connected") throw new Error("The server asked for a redirect; an outgoing webhook connects directly.");
  return res;
}

/** Replace the configuration (Edit). Secrets: a value sets, `null` deletes, an absent name is kept. */
export function useWebhookPut(connectionId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: WebhookPutBody) =>
      call<S["PutWebhookResponse"]>("/v1/connections/{id}/webhook", `/v1/connections/${encodeURIComponent(connectionId)}/webhook`, "PUT", body),
    onSuccess: (view) => {
      // The one-time signing secret never enters the cache.
      const { signing_secret: _shown, ...rest } = view;
      void _shown;
      qc.setQueryData(webhookKeys.config(connectionId), normalizeView(rest));
      void qc.invalidateQueries({ queryKey: ["accounts"] });
    },
  });
}

/** Send test (C-909): really calls the webhook, or with `dry_run` shows the stored configuration's rendering. */
export function useWebhookTest(connectionId: string) {
  return useMutation({
    mutationFn: async (v: { values: Record<string, unknown>; dry_run: boolean }): Promise<WebhookTestResult | WebhookPreview> => {
      const r = await call<S["TestResponse"]>(
        "/v1/connections/{id}/webhook/test",
        `/v1/connections/${encodeURIComponent(connectionId)}/webhook/test`,
        "POST",
        v.dry_run ? { values: v.values, dry_run: true } : { values: v.values },
      );
      if (r.status != null) {
        return {
          status: r.status,
          content_type: r.content_type ?? null,
          body: r.body ?? "",
          truncated: r.truncated ?? false,
          delivery_id: r.delivery_id ?? "",
        };
      }
      return { method: r.method ?? "", content_type: r.content_type ?? "", headers: r.headers ?? [], body: r.body ?? "" };
    },
  });
}

/** A test answer that was really sent has a status; a dry run has a method. */
export const isSent = (r: WebhookTestResult | WebhookPreview): r is WebhookTestResult => "status" in r;

// ---- The organization's switch and limits (C-912) ----

/** The organization's outgoing-webhook settings, as `PATCH /v1/orgs/{org}` keys them. */
export type OrgWebhookSettings = Pick<
  S["OrgSettingsPatch"],
  "outgoing_webhooks_enabled" | "outgoing_webhooks_per_minute_max" | "outgoing_webhooks_per_day_max"
>;
