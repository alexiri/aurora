"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type SettingsIndex = components["schemas"]["SettingsIndex"];
export type SettingsSection = components["schemas"]["SettingsSection"];
export type Setting = components["schemas"]["Setting"];
export type SettingEntry = components["schemas"]["SettingEntry"];
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
export function sectionSlug(section: string): string {
  return section.replace(/_/g, "-");
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

/** What a save would change, which values loosen and which organizations are affected (C-774). */
export function usePreviewSettings(section: string) {
  return useMutation({
    mutationFn: (body: { generation: number; values: SettingValues }) =>
      unwrap(
        api.POST("/v1/admin/settings/{section}/preview", {
          params: { path: { section } },
          body: body as never,
        }),
      ),
  });
}

/** Replace a section's dashboard values (C-773). */
export function usePutSettings(section: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { generation: number; values: SettingValues }) =>
      unwrap(
        api.PUT("/v1/admin/settings/{section}", {
          params: { path: { section } },
          body: body as never,
        }),
      ),
    onSuccess: (v) => {
      qc.setQueryData([...INDEX_KEY, section], v);
      void qc.invalidateQueries({ queryKey: INDEX_KEY });
    },
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
