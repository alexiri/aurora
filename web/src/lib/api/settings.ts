"use client";

import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type SettingsIndex = components["schemas"]["SettingsIndex"];
export type SettingsSection = components["schemas"]["SettingsSection"];
export type Setting = components["schemas"]["Setting"];
export type SettingEntry = components["schemas"]["SettingEntry"];
/** A settings field in words (C-972) and a read-only current value (C-973). */
export type FieldInfo = components["schemas"]["FieldInfo"];
export type CurrentValue = components["schemas"]["CurrentValue"];
export type SettingsPreview = components["schemas"]["SettingsPreview"];
export type ImportResult = components["schemas"]["ImportResult"];
export type ImportRequest = components["schemas"]["ImportRequest"];
export type DiffItem = components["schemas"]["DiffItem"];

/** The values sent by a PUT: dotted key → value, secrets as `{ $secret: … }` (C-773). */
export type SettingValues = Record<string, unknown>;

/** Section ids appear in URLs with `-` for `_` (`/admin/settings/sign-in`, `14`). */
export function sectionFromSlug(slug: string): string {
  return slug.replace(/-/g, "_");
}

const INDEX_KEY = ["admin", "settings"] as const;

/** The sections and the keys the deployment configuration locks (C-771). */
export function useSettingsIndex() {
  return useQuery({
    queryKey: INDEX_KEY,
    queryFn: () => unwrap(api.GET("/v1/admin/settings")),
    retry: false,
  });
}

/** One section with every setting's effective value, source and lock (C-772). */
export function useSettingsSection(section: string) {
  return useQuery({
    queryKey: [...INDEX_KEY, section] as const,
    queryFn: () => unwrap(api.GET("/v1/admin/settings/{section}", { params: { path: { section } } })),
    retry: false,
  });
}

export type GovernanceOrgRow = components["schemas"]["OrgStandingRow"];

/** Where organizations stand against the instance governance ceiling (C-778), newest first. */
export function useGovernanceOrgs(cursor?: string) {
  return useQuery({
    queryKey: [...INDEX_KEY, "governance", "orgs", cursor ?? ""] as const,
    queryFn: () =>
      unwrap(api.GET("/v1/admin/settings/governance/orgs", { params: { query: { limit: 50, cursor } } })),
    retry: false,
  });
}

/**
 * Preview and replace any section by id (P5-IA-3: a stacked page saves several sections from one Save, so the section
 * is an argument rather than a hook parameter).
 */
export function useSettingsSaver() {
  const qc = useQueryClient();
  return {
    preview: (section: string, body: { generation: number; values: SettingValues }) =>
      unwrap(api.POST("/v1/admin/settings/{section}/preview", { params: { path: { section } }, body: body as never })),
    put: async (section: string, body: { generation: number; values: SettingValues }) => {
      const v = await unwrap(api.PUT("/v1/admin/settings/{section}", { params: { path: { section } }, body: body as never }));
      qc.setQueryData([...INDEX_KEY, section], v);
      void qc.invalidateQueries({ queryKey: INDEX_KEY });
      return v;
    },
  };
}

/** Several sections at once, in the order asked (the stacked pages). */
export function useSettingsSections(sections: string[]) {
  return useQueries({
    queries: sections.map((section) => ({
      queryKey: [...INDEX_KEY, section] as const,
      queryFn: () => unwrap(api.GET("/v1/admin/settings/{section}", { params: { path: { section } } })),
      retry: false,
    })),
  });
}

export type LlmEndpointTestResult = components["schemas"]["EndpointTestResult"];

/** One unauthenticated request to an endpoint's models path from the API (C-779): nothing is saved. */
export function useTestLlmEndpoint() {
  return useMutation({
    mutationFn: (endpoint: Record<string, unknown>) =>
      unwrap(api.POST("/v1/admin/settings/llm/endpoint-test", { body: { endpoint } as never })),
  });
}

export type ProviderTestResult = components["schemas"]["ProviderTestResult"];
export type EmailTestResult = components["schemas"]["EmailTestResult"];

/** Discovery against a provider's issuer, from the API (C-780): nothing is saved, no secret is needed. */
export function useTestProvider() {
  return useMutation({
    mutationFn: (provider: Record<string, unknown>) =>
      unwrap(api.POST("/v1/admin/settings/sign_in/provider-test", { body: { provider } as never })),
  });
}

/** One test e-mail to the caller's own address (C-781); `values` = the unsaved section values, absent = in force. */
export function useTestEmail() {
  return useMutation({
    mutationFn: (values: SettingValues | null) =>
      unwrap(api.POST("/v1/admin/settings/email/test", { body: (values ? { values } : {}) as never })),
  });
}

/** Import a settings document: a dry run shows the diff, applying writes every section (C-776). */
export function useImportSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: ImportRequest) => unwrap(api.POST("/v1/admin/settings/import", { body })),
    onSuccess: (v) => {
      if (v.applied) void qc.invalidateQueries({ queryKey: INDEX_KEY });
    },
  });
}

/** The export download URL (C-775): a plain GET with the session cookie. */
export function exportUrl(format: "yaml" | "json"): string {
  return `/v1/admin/settings/export?format=${format}`;
}
