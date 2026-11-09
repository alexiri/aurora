"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Skill = components["schemas"]["Skill"];
export type SkillFile = components["schemas"]["SkillFile"];
export type CreateSkillRequest = components["schemas"]["CreateSkillRequest"];
export type UpdateSkillRequest = components["schemas"]["UpdateSkillRequest"];

/** Limits from docs/02; the API stays authoritative. */
export const SKILL_NAME_MAX = 64;
export const SKILL_DESCRIPTION_MAX = 500;
export const SKILL_BODY_MAX = 24_000;
export const SKILL_TOTAL_MAX = 32_000;
export const SKILL_PATH_MAX_DEPTH = 8;

/** Returns an error message, or null when the relative file path is acceptable. */
export function validateSkillPath(path: string): string | null {
  if (!path) return "Enter a file path.";
  if (path.startsWith("/") || path.includes("\\")) return "Use a relative path with forward slashes.";
  const parts = path.split("/");
  if (parts.some((p) => p === "" || p === "." || p === "..")) return "The path must not contain empty, '.' or '..' segments.";
  if (parts.length > SKILL_PATH_MAX_DEPTH) return `The path may have at most ${SKILL_PATH_MAX_DEPTH} levels.`;
  if (/[\u0000-\u001f]/.test(path)) return "The path contains control characters.";
  if (path === "SKILL.md") return "SKILL.md is the skill body; edit it above.";
  return null;
}

export function validateSkillName(name: string): string | null {
  if (!name || name.length > SKILL_NAME_MAX || !/^[a-z0-9]+(-[a-z0-9]+)*$/.test(name))
    return `Use lowercase words joined by hyphens, at most ${SKILL_NAME_MAX} characters.`;
  if (name.startsWith("aurora-")) return "Names starting with aurora- are reserved.";
  return null;
}

export type FileTreeNode = { name: string; path: string; file?: SkillFile; children: FileTreeNode[] };

/** Builds a nested tree (directories first, then alphabetical) from flat file paths. */
export function buildTree(files: SkillFile[]): FileTreeNode[] {
  const root: FileTreeNode = { name: "", path: "", children: [] };
  for (const f of files) {
    let node = root;
    const parts = f.path.split("/");
    parts.forEach((part, i) => {
      const path = parts.slice(0, i + 1).join("/");
      let next = node.children.find((c) => c.name === part);
      if (!next) {
        next = { name: part, path, children: [] };
        node.children.push(next);
      }
      if (i === parts.length - 1) next.file = f;
      node = next;
    });
  }
  const sort = (n: FileTreeNode[]) => {
    n.sort((a, b) => Number(!!a.file) - Number(!!b.file) || a.name.localeCompare(b.name));
    n.forEach((c) => sort(c.children));
  };
  sort(root.children);
  return root.children;
}

export type EffectiveSkill = components["schemas"]["EffectiveSkill"];
export type ShadowedSkill = components["schemas"]["ShadowedSkill"];
export type SkillTier = Skill["tier"];

/** Display names of the tiers; precedence is agent > workspace > organization (D-64). */
export const TIER_LABEL: Record<SkillTier, string> = { agent: "Agent", workspace: "Workspace", org: "Organization" };

export const skillKeys = {
  agent: (agent: string) => ["agents", agent, "skills"] as const,
  org: (org: string) => ["orgs", org, "skills"] as const,
  workspace: (ws: string) => ["workspaces", ws, "skills"] as const,
  effective: (agent: string) => ["agents", agent, "skills", "effective"] as const,
};

export function useWorkspaceSkills(ws: string | undefined) {
  return useQuery({
    queryKey: skillKeys.workspace(ws ?? ""),
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/skills", { params: { path: { ws: ws! } } })),
    enabled: !!ws,
    retry: false,
  });
}

/** The set an agent receives (one skill per name) plus the skills that do not reach it. */
export function useEffectiveSkills(agent: string) {
  return useQuery({
    queryKey: skillKeys.effective(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/skills/effective", { params: { path: { agent } } })),
    retry: false,
  });
}

export function useAgentSkills(agent: string) {
  return useQuery({
    queryKey: skillKeys.agent(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/skills", { params: { path: { agent } } })),
    retry: false,
  });
}

export function useOrgSkills(org: string | undefined) {
  return useQuery({
    queryKey: skillKeys.org(org ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/skills", { params: { path: { org: org! } } })),
    enabled: !!org,
    retry: false,
  });
}

/** Scope selects which list is invalidated; `scopeId` is the agent or org id. */
export type SkillScope = { kind: "agent" | "workspace" | "organization"; id: string };

function useInvalidateSkills(scope: SkillScope) {
  const qc = useQueryClient();
  return () => {
    const key =
      scope.kind === "agent" ? skillKeys.agent(scope.id) : scope.kind === "workspace" ? skillKeys.workspace(scope.id) : skillKeys.org(scope.id);
    // Any tier change can alter what an agent receives: refresh every effective-set view too.
    void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === "agents" && q.queryKey[3] === "effective" });
    return qc.invalidateQueries({ queryKey: key });
  };
}

export function useCreateSkill(scope: SkillScope) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: (body: CreateSkillRequest) =>
      scope.kind === "agent"
        ? unwrap(api.POST("/v1/agents/{agent}/skills", { params: { path: { agent: scope.id } }, body }))
        : scope.kind === "workspace"
          ? unwrap(api.POST("/v1/workspaces/{ws}/skills", { params: { path: { ws: scope.id } }, body }))
          : unwrap(api.POST("/v1/orgs/{org}/skills", { params: { path: { org: scope.id } }, body })),
    onSuccess: inv,
  });
}

export function useUpdateSkill(scope: SkillScope, id: string) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: (body: UpdateSkillRequest) =>
      unwrap(api.PATCH("/v1/skills/{id}", { params: { path: { id } }, body })),
    onSuccess: inv,
  });
}

export function useDeleteSkill(scope: SkillScope) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/skills/{id}", { params: { path: { id } } })),
    onSuccess: inv,
  });
}

/**
 * The generated template is `/v1/skills/{id}/files/{*path}`, so openapi-fetch looks the value up under the
 * key `*path` (the typed key `path` would leave the placeholder unreplaced).
 */
function skillFilePathParams(id: string, path: string) {
  return { id, "*path": path } as unknown as { id: string; path: string };
}

/** Raw-body upload; `executable` omitted keeps the current bit. */
export function usePutSkillFile(scope: SkillScope, id: string) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: ({ path, content, executable }: { path: string; content: Uint8Array; executable?: boolean }) =>
      unwrap(
        api.PUT("/v1/skills/{id}/files/{*path}", {
          params: { path: skillFilePathParams(id, path), query: executable === undefined ? {} : { executable } },
          body: content as unknown as number[],
          bodySerializer: (b: unknown) => b as BodyInit,
          headers: { "content-type": "application/octet-stream" },
        }),
      ),
    onSuccess: inv,
  });
}

export function useDeleteSkillFile(scope: SkillScope, id: string) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: (path: string) =>
      unwrap(api.DELETE("/v1/skills/{id}/files/{*path}", { params: { path: skillFilePathParams(id, path) } })),
    onSuccess: inv,
  });
}
