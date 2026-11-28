"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Skill = components["schemas"]["Skill"];
export type SkillFile = components["schemas"]["SkillFile"];
export type CreateSkillRequest = components["schemas"]["CreateSkillRequest"];
export type UpdateSkillRequest = components["schemas"]["UpdateSkillRequest"];

/** Limits from docs/02; the API stays authoritative. */
export const SKILL_NAME_MAX = 64;
export const SKILL_DESCRIPTION_MAX = 1_024;
export const SKILL_BODY_MAX = 48_000;
export const SKILL_TOTAL_MAX = 300_000;
export const SKILL_TITLE_MAX = 80;
export const SKILL_PATH_MAX_DEPTH = 4;
export const SKILL_PATH_MAX_LENGTH = 200;
/** Files beside `SKILL.md`. */
export const SKILL_FILES_MAX = 50;

/** Returns an error message, or null when the relative file path is acceptable (the server's own rules). */
export function validateSkillPath(path: string): string | null {
  if (!path) return "Enter a file name.";
  if (path.startsWith("/") || path.includes("\\")) return "Use a relative path with forward slashes.";
  const parts = path.split("/");
  if (parts.some((p) => p === "" || p === "." || p === "..")) return "The path must not contain empty, '.' or '..' segments.";
  if (parts.length > SKILL_PATH_MAX_DEPTH) return `The path may have at most ${SKILL_PATH_MAX_DEPTH} levels.`;
  if (!/^[A-Za-z0-9._/-]+$/.test(path)) return "Use only letters, digits, dots, dashes, underscores and slashes.";
  if (path.length > SKILL_PATH_MAX_LENGTH) return `The path may have at most ${SKILL_PATH_MAX_LENGTH} characters.`;
  if (parts.some((p) => p.toLowerCase() === "skill.md")) return "SKILL.md is the skill's instructions; edit them above.";
  return null;
}

/** The bytes as text, or null when they are not: not UTF-8, or holding a NUL. */
export function decodeSkillText(bytes: Uint8Array): string | null {
  try {
    const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
    return text.includes("\0") ? null : text;
  } catch {
    return null;
  }
}

/** A file's size as the budget counts it: characters for text, base64 length for binary content. */
export function skillContentSize(bytes: Uint8Array): number {
  const text = decodeSkillText(bytes);
  return text === null ? 4 * Math.ceil(bytes.length / 3) : [...text].length;
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
export function useEffectiveSkills(agent: string, enabled = true) {
  return useQuery({
    queryKey: skillKeys.effective(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/skills/effective", { params: { path: { agent } } })),
    enabled: enabled && agent !== "",
    retry: false,
  });
}

export function useAgentSkills(agent: string) {
  return useQuery({
    queryKey: skillKeys.agent(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/skills", { params: { path: { agent } } })),
    // A workspace or organization list passes "": no request then (was GET /v1/agents//skills, 400).
    enabled: agent !== "",
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
    // Any tier change can alter what an agent receives: refresh every effective-set view too, before the write is done
    // (the agent's page lists the effective set, so a new skill must be in it when the panel opens it).
    return Promise.all([
      qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === "agents" && q.queryKey[3] === "effective" }),
      qc.invalidateQueries({ queryKey: key }),
    ]);
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

/** One file of a skill as stored (C-1099): the bytes, for the agent export. */
export async function readSkillFile(id: string, path: string): Promise<Uint8Array> {
  const { data, error, response } = await api.GET("/v1/skills/{id}/files/{*path}", {
    params: { path: skillFilePathParams(id, path) },
    parseAs: "arrayBuffer",
  });
  if (!response.ok) throw ApiError.fromBody(error, response.status);
  return new Uint8Array(data as ArrayBuffer);
}

/** Raw-body upload; `executable` omitted keeps the current bit. */
export function putSkillFile(id: string, path: string, content: Uint8Array, executable?: boolean) {
  return unwrap(
    api.PUT("/v1/skills/{id}/files/{*path}", {
      params: { path: skillFilePathParams(id, path), query: executable === undefined ? {} : { executable } },
      body: content as unknown as number[],
      bodySerializer: (b: unknown) => b as BodyInit,
      headers: { "content-type": "application/octet-stream" },
    }),
  );
}

export function usePutSkillFile(scope: SkillScope, id: string) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: ({ path, content, executable }: { path: string; content: Uint8Array; executable?: boolean }) =>
      putSkillFile(id, path, content, executable),
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

export interface NewSkillFile {
  path: string;
  content: Uint8Array;
  executable: boolean;
}

/** A file of a new skill could not be written; the half-made skill was removed. */
export class SkillFileError extends Error {
  constructor(
    readonly path: string,
    readonly reason: unknown,
  ) {
    super(path);
  }
}

/**
 * A new skill with its files, through the ordinary routes. With files it is created switched off and switched on only
 * once every file is there, so no agent ever receives half a skill; when a file is refused, the skill is removed again
 * and the error names the file.
 */
export function useCreateSkillWithFiles(scope: SkillScope) {
  const inv = useInvalidateSkills(scope);
  const create = useCreateSkill(scope);
  return useMutation({
    mutationFn: async ({ skill, files }: { skill: CreateSkillRequest; files: NewSkillFile[] }): Promise<Skill> => {
      const enabled = skill.enabled ?? true;
      const made = await create.mutateAsync({ ...skill, enabled: files.length ? false : enabled });
      if (!files.length) return made;
      let current = "";
      try {
        for (const f of files) {
          current = f.path;
          await putSkillFile(made.id, f.path, f.content, f.executable);
        }
        current = "";
        if (enabled) return await unwrap(api.PATCH("/v1/skills/{id}", { params: { path: { id: made.id } }, body: { enabled } }));
        return made;
      } catch (error) {
        await unwrap(api.DELETE("/v1/skills/{id}", { params: { path: { id: made.id } } })).catch(() => undefined);
        throw current ? new SkillFileError(current, error) : error;
      }
    },
    onSettled: inv,
  });
}

/** (C-1174) Turns a workspace or organization skill off for one agent only, or back on. */
export function useSetAgentSkillOff(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ skillId, off }: { skillId: string; off: boolean }) => {
      const params = { path: { agent, skill_id: skillId } };
      return unwrap(
        off ? api.PUT("/v1/agents/{agent}/skills/{skill_id}/off", { params }) : api.DELETE("/v1/agents/{agent}/skills/{skill_id}/off", { params }),
      );
    },
    onSettled: () => qc.invalidateQueries({ queryKey: skillKeys.effective(agent) }),
  });
}

/** Switches any stored skill on or off for its own level (a row's switch). */
export function useSetSkillEnabled(scope: SkillScope) {
  const inv = useInvalidateSkills(scope);
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      unwrap(api.PATCH("/v1/skills/{id}", { params: { path: { id } }, body: { enabled } })),
    onSuccess: inv,
  });
}
