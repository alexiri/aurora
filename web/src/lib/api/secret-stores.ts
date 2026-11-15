"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type StoresStatus = components["schemas"]["StoresStatus"];
export type StoreView = components["schemas"]["StoreView"];
export type MigrationView = components["schemas"]["MigrationView"];
export type TestResult = components["schemas"]["TestResult"];

/** A store definition as the form sends it (the entry without its `id`; secrets as `{ $secret }`). */
export type StoreInput = Record<string, unknown>;

const KEY = ["admin", "secret-stores"] as const;

/** The store registry with health, counts and the migration state (C-746); polls while a job runs. */
export function useSecretStores() {
  return useQuery({
    queryKey: KEY,
    queryFn: () => unwrap(api.GET("/v1/admin/secret-stores")),
    retry: false,
    refetchInterval: (q) => (q.state.data?.migration.state === "running" ? 1500 : false),
  });
}

function useWrite<V>(fn: (v: V) => Promise<StoresStatus>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (v) => {
      qc.setQueryData(KEY, v);
      void qc.invalidateQueries({ queryKey: ["admin", "settings"] });
    },
  });
}

/** Add or replace a dashboard-defined store. */
export function usePutStore() {
  return useWrite((v: { id: string; generation: number; store: StoreInput }) =>
    unwrap(
      api.PUT("/v1/admin/secret-stores/{id}", {
        params: { path: { id: v.id } },
        body: { generation: v.generation, store: v.store as never },
      }),
    ),
  );
}

export function useDeleteStore() {
  return useWrite((v: { id: string; generation: number }) =>
    unwrap(
      api.DELETE("/v1/admin/secret-stores/{id}", {
        params: { path: { id: v.id }, query: { generation: v.generation } },
      }),
    ),
  );
}

export function usePutDefaultStore() {
  return useWrite((v: { storeId: string; generation: number }) =>
    unwrap(
      api.PUT("/v1/admin/secret-store-default", {
        body: { generation: v.generation, store_id: v.storeId },
      }),
    ),
  );
}

export function usePutScopes() {
  return useWrite((v: { id: string; generation: number; scopes: { org_id: string; paths: string[] }[] }) =>
    unwrap(
      api.PUT("/v1/admin/secret-stores/{id}/reference-scopes", {
        params: { path: { id: v.id } },
        body: { generation: v.generation, scopes: v.scopes },
      }),
    ),
  );
}

/** Try a definition before saving it: nothing is stored. */
export function useTestStore() {
  return useMutation({
    mutationFn: (v: { id: string; store: StoreInput }) =>
      unwrap(api.POST("/v1/admin/secret-store-test", { body: { id: v.id, store: v.store as never } })),
  });
}

/** Dry run (the counts) or start the job that moves every managed value to `target`. */
export function useStartMigration() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { target: string; dryRun: boolean }) =>
      unwrap(api.POST("/v1/admin/secret-stores/migrations", { body: { target: v.target, dry_run: v.dryRun } })),
    onSuccess: (_v, vars) => {
      if (!vars.dryRun) void qc.invalidateQueries({ queryKey: KEY });
    },
  });
}
