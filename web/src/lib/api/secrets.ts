"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Secret = components["schemas"]["Secret"];
export type InjectionRecipe = components["schemas"]["InjectionRecipe"];
export type CreateSecretRequest = components["schemas"]["CreateSecretRequest"];
export type PatchSecretRequest = components["schemas"]["PatchSecretRequest"];
export type SetValueRequest = components["schemas"]["SetValueRequest"];
export type SecretReference = components["schemas"]["SecretReference"];
export type LlmEndpointInfo = components["schemas"]["LlmEndpointInfo"];

/** Where credentials live: a workspace (`ws`) or an organization. */
export type SecretScope = { kind: "workspace"; id: string } | { kind: "organization"; id: string };

export const secretKeys = {
  list: (scope: SecretScope) => ["secrets", scope.kind, scope.id] as const,
  stores: ["secret-stores"] as const,
  providers: (orgId: string) => ["llm-providers", orgId] as const,
};

export function useSecrets(scope: SecretScope) {
  return useQuery({
    queryKey: secretKeys.list(scope),
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/secrets", { params: { path: { ws: scope.id } } }))
        : unwrap(api.GET("/v1/orgs/{org}/secrets", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

export function useSecretStores() {
  return useQuery({
    queryKey: secretKeys.stores,
    queryFn: () => unwrap(api.GET("/v1/secret-stores")),
    retry: false,
  });
}

export function useLlmProviders(orgId: string | undefined) {
  return useQuery({
    queryKey: secretKeys.providers(orgId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/llm/providers", { params: { query: { org_id: orgId! } } })),
    enabled: !!orgId,
    retry: false,
  });
}

export function useCreateSecret(scope: SecretScope) {
  const qc = useQueryClient();
  return useMutation({
    // The plaintext is in `variables`: drop the mutation as soon as it settles (WEBC-4).
    gcTime: 0,
    mutationFn: (body: CreateSecretRequest) =>
      scope.kind === "workspace"
        ? unwrap(api.POST("/v1/workspaces/{ws}/secrets", { params: { path: { ws: scope.id } }, body }))
        : unwrap(api.POST("/v1/orgs/{org}/secrets", { params: { path: { org: scope.id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function usePatchSecret(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchSecretRequest) =>
      unwrap(api.PATCH("/v1/secrets/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function useSetSecretValue(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: SetValueRequest) =>
      unwrap(api.PUT("/v1/secrets/{id}/value", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function useVerifySecret(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.POST("/v1/secrets/{id}/verify", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function useDeleteSecret(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { error, response } = await api.DELETE("/v1/secrets/{id}", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function useAcceptSecret(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (value: string) =>
      unwrap(api.POST("/v1/secrets/{id}/accept", { params: { path: { id } }, body: { value } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

export function useRejectSecret(scope: SecretScope, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { error, response } = await api.POST("/v1/secrets/{id}/reject", { params: { path: { id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.list(scope) }),
  });
}

// ---- pure form helpers (unit tested) ----

export type RecipeKind = InjectionRecipe["kind"];
export const RECIPE_KINDS: { kind: RecipeKind; label: string }[] = [
  { kind: "bearer", label: "Bearer token (Authorization header)" },
  { kind: "header", label: "Custom header" },
  { kind: "query", label: "Query parameter" },
  { kind: "basic", label: "HTTP basic auth" },
  { kind: "path_template", label: "URL path prefix" },
  { kind: "path_regex", label: "URL path regex" },
];

export interface RecipeFields {
  name: string;
  format: string;
  username: string;
  prefix: string;
  pattern: string;
  replacement: string;
}
export const EMPTY_RECIPE_FIELDS: RecipeFields = {
  name: "", format: "", username: "", prefix: "", pattern: "", replacement: "",
};

export function buildRecipe(kind: RecipeKind, f: RecipeFields): InjectionRecipe {
  switch (kind) {
    case "bearer":
      return { kind };
    case "header":
    case "query":
      return f.format.trim()
        ? { kind, name: f.name.trim(), format: f.format.trim() }
        : { kind, name: f.name.trim() };
    case "basic":
      return { kind, username: f.username };
    case "path_template":
      return { kind, prefix: f.prefix };
    case "path_regex":
      return { kind, pattern: f.pattern, replacement: f.replacement };
  }
}

export function recipeToFields(r: InjectionRecipe): RecipeFields {
  const out = { ...EMPTY_RECIPE_FIELDS };
  if (r.kind === "header" || r.kind === "query") {
    out.name = r.name;
    out.format = r.format ?? "";
  } else if (r.kind === "basic") out.username = r.username;
  else if (r.kind === "path_template") out.prefix = r.prefix;
  else if (r.kind === "path_regex") {
    out.pattern = r.pattern;
    out.replacement = r.replacement;
  }
  return out;
}

export interface GenericFormState {
  name: string;
  host: string;
  path: string;
  kind: RecipeKind;
  fields: RecipeFields;
  source: "value" | "reference";
  value: string;
  ref: { store_id: string; path: string; field: string };
}

export function buildGenericCreate(s: GenericFormState): CreateSecretRequest {
  const body: CreateSecretRequest = {
    name: s.name.trim(),
    kind: "generic",
    host_pattern: s.host.trim(),
    injection: buildRecipe(s.kind, s.fields),
  };
  if (s.path.trim()) body.path_pattern = s.path.trim();
  if (s.source === "reference") {
    body.reference = {
      store_id: s.ref.store_id,
      path: s.ref.path.trim(),
      ...(s.ref.field.trim() ? { field: s.ref.field.trim() } : {}),
    };
  } else body.value = s.value;
  return body;
}

export function buildLlmCreate(s: {
  name: string;
  endpointId: string;
  authMode: string;
  value: string;
}): CreateSecretRequest {
  return {
    name: s.name.trim(),
    kind: "llm",
    llm: { endpoint_id: s.endpointId, ...(s.authMode ? { auth_mode: s.authMode } : {}) },
    value: s.value,
  };
}

/** Whether any store can hold references (C-07). */
export function referencesAvailable(stores: { capabilities: { references: boolean } }[] | undefined): boolean {
  return !!stores?.some((s) => s.capabilities.references);
}
