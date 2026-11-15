"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import type { SkillScope } from "./skills";
import { unwrap } from "./unwrap";

export type SkillLibrary = components["schemas"]["SkillLibrary"];
export type CreateSkillLibraryRequest = components["schemas"]["CreateSkillLibraryRequest"];
export type PatchSkillLibraryRequest = components["schemas"]["PatchSkillLibraryRequest"];
export type LibraryCatalogue = components["schemas"]["LibraryCatalogue"];
export type LibrarySkillItem = components["schemas"]["LibrarySkillItem"];
export type LibrarySkillContent = components["schemas"]["LibrarySkillContent"];
export type SkillLibraryInfo = components["schemas"]["SkillLibraryInfo"];
export type LibraryUpdate = components["schemas"]["LibraryUpdateView"];
export type LibraryDiff = components["schemas"]["LibraryDiff"];
export type PendingUpdate = components["schemas"]["PendingLibraryUpdate"];

/** `LIBRARY_BYTES_PER_*` and `LIBRARY_SKILLS_PER_*` (docs/02 §Limits); the API stays authoritative. */
export const LIBRARY_BUDGET = {
  agent: { bytes: 1_048_576, skills: 25 },
  workspace: { bytes: 2_097_152, skills: 50 },
  org: { bytes: 4_194_304, skills: 100 },
} as const;

/** The tiers that have a library budget. */
export type BudgetTier = keyof typeof LIBRARY_BUDGET;

export const libraryKeys = {
  list: (org: string) => ["orgs", org, "skill-libraries"] as const,
  one: (id: string) => ["skill-libraries", id] as const,
  catalogue: (id: string, q: string) => ["skill-libraries", id, "skills", q] as const,
  content: (id: string, path: string, commit: string) => ["skill-libraries", id, "skill", path, commit] as const,
  info: (skill: string) => ["skills", skill, "library"] as const,
  pending: (scope: SkillScope) => ["skill-library-updates", scope.kind, scope.id] as const,
};

/** Admins get every field; other members who can use a workspace get `id`, `label` and `status` of active libraries. */
export function useSkillLibraries(org: string | undefined) {
  return useQuery({
    queryKey: libraryKeys.list(org ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/skill-libraries", { params: { path: { org: org! } } })),
    enabled: !!org,
    retry: false,
  });
}

function useInvalidateLibraries(org: string) {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: libraryKeys.list(org) });
}

export function useCreateSkillLibrary(org: string) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: (body: CreateSkillLibraryRequest) => unwrap(api.POST("/v1/orgs/{org}/skill-libraries", { params: { path: { org } }, body })),
    onSuccess: inv,
  });
}

export function usePatchSkillLibrary(org: string) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: PatchSkillLibraryRequest }) =>
      unwrap(api.PATCH("/v1/skill-libraries/{id}", { params: { path: { id } }, body })),
    onSuccess: inv,
  });
}

export function useDeleteSkillLibrary(org: string) {
  const qc = useQueryClient();
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/skill-libraries/{id}", { params: { path: { id } } })),
    onSuccess: async () => {
      // Skills added from the library are detached: every skill list changes.
      await qc.invalidateQueries({ predicate: (q) => q.queryKey.includes("skills") });
      await inv();
    },
  });
}

export function useCheckLibraryNow(org: string) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/skill-libraries/{id}/refresh", { params: { path: { id } } })),
    onSuccess: inv,
  });
}

/** The library's current head, live (cached 5 minutes server-side). Nothing is stored by browsing. */
export function useLibraryCatalogue(id: string | undefined, q: string) {
  return useInfiniteQuery({
    queryKey: libraryKeys.catalogue(id ?? "", q),
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/skill-libraries/{id}/skills", {
          params: { path: { id: id! }, query: { q: q || undefined, cursor: pageParam || undefined } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: !!id,
    retry: false,
    staleTime: 60_000,
  });
}

/** Exactly what an add would store, at one commit. */
export function useLibrarySkill(id: string | undefined, path: string | undefined, commit: string | undefined) {
  return useQuery({
    queryKey: libraryKeys.content(id ?? "", path ?? "", commit ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/v1/skill-libraries/{id}/skills/{path}", {
          params: { path: { id: id!, path: path! }, query: { commit_sha: commit } },
        }),
      ),
    enabled: !!id && !!path && !!commit,
    retry: false,
  });
}

/** The pin of a library skill and its open proposed update with the whole diff. */
export function useSkillLibraryInfo(skill: string | undefined) {
  return useQuery({
    queryKey: libraryKeys.info(skill ?? ""),
    queryFn: () => unwrap(api.GET("/v1/skills/{id}/library", { params: { path: { id: skill! } } })),
    enabled: !!skill,
    retry: false,
  });
}

/** Pending proposed updates the caller may decide at one scope (badges, "Update available"). */
export function usePendingLibraryUpdates(scope: SkillScope, enabled = true) {
  return useQuery({
    queryKey: libraryKeys.pending(scope),
    queryFn: () =>
      scope.kind === "agent"
        ? unwrap(api.GET("/v1/agents/{agent}/skill-library-updates", { params: { path: { agent: scope.id } } }))
        : scope.kind === "workspace"
          ? unwrap(api.GET("/v1/workspaces/{ws}/skill-library-updates", { params: { path: { ws: scope.id } } }))
          : unwrap(api.GET("/v1/orgs/{org}/skill-library-updates", { params: { path: { org: scope.id } } })),
    enabled,
    retry: false,
  });
}

function useRefreshAfterDecision(skill: string) {
  const qc = useQueryClient();
  return async () => {
    await qc.invalidateQueries({ queryKey: libraryKeys.info(skill) });
    await qc.invalidateQueries({ queryKey: ["skill-library-updates"] });
    await qc.invalidateQueries({ predicate: (q) => q.queryKey.includes("skills") });
    await qc.invalidateQueries({ queryKey: ["orgs"] });
  };
}

/** Approve exactly the content that was reviewed (the proposal's `content_sha256`). */
export function useApproveLibraryUpdate(skill: string) {
  const done = useRefreshAfterDecision(skill);
  return useMutation({
    mutationFn: ({ update, contentSha256 }: { update: string; contentSha256: string }) =>
      unwrap(
        api.POST("/v1/skills/{id}/library/updates/{uid}/approve", {
          params: { path: { id: skill, uid: update } },
          body: { content_sha256: contentSha256 },
        }),
      ),
    onSuccess: done,
  });
}

export function useRejectLibraryUpdate(skill: string) {
  const done = useRefreshAfterDecision(skill);
  return useMutation({
    mutationFn: (update: string) =>
      unwrap(api.POST("/v1/skills/{id}/library/updates/{uid}/reject", { params: { path: { id: skill, uid: update } } })),
    onSuccess: done,
  });
}

export function useDetachLibrarySkill(skill: string) {
  const done = useRefreshAfterDecision(skill);
  return useMutation({
    mutationFn: () => unwrap(api.POST("/v1/skills/{id}/library/detach", { params: { path: { id: skill } } })),
    onSuccess: done,
  });
}

/** `owner/name` of a repository and the first 7 characters of a commit, for badges. */
export function shortSha(sha: string | null | undefined): string {
  return (sha ?? "").slice(0, 7);
}

// ---- line diff -------------------------------------------------------------------------------

export type DiffLine = { kind: "same" | "add" | "del"; text: string };

/**
 * Whole-text line diff (longest common subsequence). Used for the update review: the reviewer reads
 * every line, nothing is summarized or truncated. Falls back to "all removed, then all added" past a
 * size where the quadratic table would be unreasonable (skills are at most 32 000 characters).
 */
export function diffLines(oldText: string, newText: string): DiffLine[] {
  const full = (t: string) => (t === "" ? [] : t.split("\n"));
  const a0 = full(oldText);
  const b0 = full(newText);
  // Lines both versions share at the start and the end are identical context: only the middle needs the table.
  let head = 0;
  while (head < a0.length && head < b0.length && a0[head] === b0[head]) head++;
  let tail = 0;
  while (tail < a0.length - head && tail < b0.length - head && a0[a0.length - 1 - tail] === b0[b0.length - 1 - tail]) tail++;
  const a = a0.slice(head, a0.length - tail);
  const b = b0.slice(head, b0.length - tail);
  const same = (xs: string[]): DiffLine[] => xs.map((text) => ({ kind: "same" as const, text }));
  return [...same(a0.slice(0, head)), ...middle(a, b), ...same(a0.slice(a0.length - tail))];
}

function middle(a: string[], b: string[]): DiffLine[] {
  if (a.length * b.length > 4_000_000) {
    return [...a.map((t) => ({ kind: "del" as const, text: t })), ...b.map((t) => ({ kind: "add" as const, text: t }))];
  }
  const dp: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: "same", text: a[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) out.push({ kind: "del", text: a[i++] });
    else out.push({ kind: "add", text: b[j++] });
  }
  while (i < a.length) out.push({ kind: "del", text: a[i++] });
  while (j < b.length) out.push({ kind: "add", text: b[j++] });
  return out;
}
