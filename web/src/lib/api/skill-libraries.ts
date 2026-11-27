"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type Schemas = components["schemas"];

// FIX-278's contract, kept here until `schema.d.ts` has it: regenerate the schema and drop these local additions.
/** Why a library skill is invalid, or couldn't be judged; the words are in `lib/labels.ts`. */
export type SkillInvalidReason =
  | "too_large"
  | "body_too_large"
  | "file_too_large"
  | "too_many_files"
  | "unsupported_path"
  | "unsupported_entry"
  | "skill_md"
  | "bad_name"
  | "unknown_size";
type WithReasonCode = { reason_code?: SkillInvalidReason | null };

/** `default_branch` is the branch a library with no `ref` follows; null when the server doesn't know it yet. */
export type SkillLibrary = Schemas["SkillLibrary"] & { default_branch?: string | null };
export type CreateSkillLibraryRequest = Schemas["CreateSkillLibraryRequest"];
export type PatchSkillLibraryRequest = Schemas["PatchSkillLibraryRequest"];
/** `size` is null only for a skill the server couldn't judge within its fetch caps. */
export type LibrarySkillItem = Omit<Schemas["LibrarySkillItem"], "size"> & WithReasonCode & { size: number | null };
export type LibraryCatalogue = Omit<Schemas["LibraryCatalogue"], "items"> & { items: LibrarySkillItem[] };
export type LibrarySkillContent = Schemas["LibrarySkillContent"] & WithReasonCode;
export type LibraryUpdate = Schemas["LibraryUpdateView"] & WithReasonCode;
export type SkillLibraryInfo = Omit<Schemas["SkillLibraryInfo"], "update"> & { update?: LibraryUpdate | null };
export type LibraryDiff = Schemas["LibraryDiff"];

/** The tiers that keep skills, each with its own library budget (the server says how large, C-1176). */
export type BudgetTier = "agent" | "workspace" | "org";
export type LibraryBudget = components["schemas"]["LibraryBudget"];

/** (C-1176) Where a browsed skill would go, so that the listing says what can't be added there and the room left. */
export type BrowseFor = { tier: "agent"; agent: string } | { tier: "workspace"; workspace: string } | { tier: "organization" };

export type CheckLibraryRequest = components["schemas"]["CheckSkillLibraryRequest"];
export type LibraryCheck = components["schemas"]["LibraryCheck"];

/** Where a library lives: an organization's id, or `null` for the instance's own (P5-SKLIB-3, C-1129). */
export type LibraryOwner = string | null;

export const libraryKeys = {
  list: (org: string) => ["orgs", org, "skill-libraries"] as const,
  instance: () => ["admin", "skill-libraries"] as const,
  one: (id: string) => ["skill-libraries", id] as const,
  catalogue: (id: string, q: string, org: string, target: BrowseFor | null) => ["skill-libraries", id, "skills", q, org, target] as const,
  content: (id: string, path: string, commit: string, org: string) => ["skill-libraries", id, "skill", path, commit, org] as const,
  info: (skill: string) => ["skills", skill, "library"] as const,
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

/** The instance's own libraries with `orgs_using` (instance admins, session only; C-1128). */
export function useInstanceSkillLibraries() {
  return useQuery({
    queryKey: libraryKeys.instance(),
    queryFn: () => unwrap(api.GET("/v1/admin/skill-libraries")),
    retry: false,
  });
}

/** An organization's list and the instance's own: one change can show in either (an instance library is in every organization's list). */
function useInvalidateLibraries(org: LibraryOwner) {
  const qc = useQueryClient();
  return async () => {
    await qc.invalidateQueries({ queryKey: org ? libraryKeys.list(org) : libraryKeys.instance() });
    // An instance library also sits in every organization's list.
    if (!org) await qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === "orgs" && q.queryKey[2] === "skill-libraries" });
  };
}

/** Reads the repository first and stores nothing (C-1124). */
export function useCheckLibrary(org: LibraryOwner) {
  return useMutation({
    mutationFn: (body: CheckLibraryRequest) =>
      unwrap(org ? api.POST("/v1/orgs/{org}/skill-libraries/check", { params: { path: { org } }, body }) : api.POST("/v1/admin/skill-libraries/check", { body })),
  });
}

export function useCreateSkillLibrary(org: LibraryOwner) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: (body: CreateSkillLibraryRequest) =>
      unwrap(org ? api.POST("/v1/orgs/{org}/skill-libraries", { params: { path: { org } }, body }) : api.POST("/v1/admin/skill-libraries", { body })),
    onSuccess: inv,
  });
}

export function usePatchSkillLibrary(org: LibraryOwner) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: PatchSkillLibraryRequest }) =>
      unwrap(api.PATCH("/v1/skill-libraries/{id}", { params: { path: { id } }, body })),
    onSuccess: inv,
  });
}

export function useDeleteSkillLibrary(org: LibraryOwner) {
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

export function useCheckLibraryNow(org: LibraryOwner) {
  const inv = useInvalidateLibraries(org);
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/skill-libraries/{id}/refresh", { params: { path: { id } } })),
    onSuccess: inv,
  });
}

/**
 * The library's current head, live (cached 5 minutes server-side). Nothing is stored by browsing. `org` is the
 * organization to browse as: required for an instance library (C-1128), harmless for an organization's own.
 */
export function useLibraryCatalogue(id: string | undefined, q: string, org: string | undefined, target?: BrowseFor) {
  return useInfiniteQuery({
    queryKey: libraryKeys.catalogue(id ?? "", q, org ?? "", target ?? null),
    initialPageParam: "",
    queryFn: ({ pageParam }): Promise<LibraryCatalogue> =>
      unwrap(
        api.GET("/v1/skill-libraries/{id}/skills", {
          params: { path: { id: id! }, query: { q: q || undefined, cursor: pageParam || undefined, org, ...target } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: !!id,
    retry: false,
    staleTime: 60_000,
  });
}

/** Exactly what an add would store, at one commit. */
export function useLibrarySkill(id: string | undefined, path: string | undefined, commit: string | undefined, org: string | undefined) {
  return useQuery({
    queryKey: libraryKeys.content(id ?? "", path ?? "", commit ?? "", org ?? ""),
    queryFn: (): Promise<LibrarySkillContent> =>
      unwrap(
        api.GET("/v1/skill-libraries/{id}/skills/{path}", {
          params: { path: { id: id!, path: path! }, query: { commit_sha: commit, org } },
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
    queryFn: (): Promise<SkillLibraryInfo> => unwrap(api.GET("/v1/skills/{id}/library", { params: { path: { id: skill! } } })),
    enabled: !!skill,
    retry: false,
  });
}

function useRefreshAfterDecision(skill: string) {
  const qc = useQueryClient();
  return async () => {
    await qc.invalidateQueries({ queryKey: libraryKeys.info(skill) });
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
