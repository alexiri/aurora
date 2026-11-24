"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AgentTemplateSource = components["schemas"]["AgentTemplateSource"];
export type AgentTemplateSourceList = components["schemas"]["AgentTemplateSourceList"];
export type PreviewTemplateSourceRequest = components["schemas"]["PreviewTemplateSourceRequest"];
export type TemplateSourcePreview = components["schemas"]["AgentTemplateSourcePreview"];
export type CreateTemplateSourceRequest = components["schemas"]["AddTemplateSourceRequest"];
export type UpdateTemplateSourceRequest = components["schemas"]["UpdateTemplateSourceRequest"];
export type TemplateSourceCheckResult = components["schemas"]["TemplateSourceCheckResult"];
export type AdminTemplateView = components["schemas"]["AdminTemplateView"];
export type AdminTemplateList = components["schemas"]["AdminTemplateList"];
export type UpdateTemplateRequest = components["schemas"]["UpdateTemplateRequest"];
export type TemplateVersionDetail = components["schemas"]["TemplateVersionDetail"];
export type TemplateContentView = components["schemas"]["TemplateContentView"];
export type FileView = components["schemas"]["FileView"];
export type OrgTemplateItem = components["schemas"]["OrgTemplateItem"];

// ---- Workspace catalogue, plan and create (C-1110, C-1111, C-1116) -----------------------------

export type TemplateCard = components["schemas"]["TemplateCard"];
export type TemplateCardList = components["schemas"]["TemplateCardList"];
export type TemplatePlan = components["schemas"]["TemplatePlan"];
export type PlanEntry = components["schemas"]["PlanEntry"];
export type PlanChoice = components["schemas"]["PlanChoice"];
export type SkillRefPlan = components["schemas"]["SkillRefPlan"];
export type CreateFromTemplateRequest = components["schemas"]["CreateFromTemplateRequest"];
export type CreatedFromTemplate = components["schemas"]["CreatedFromTemplate"];

export const workspaceTemplateKeys = {
  list: (ws: string) => ["workspaces", ws, "agent-templates"] as const,
  plan: (ws: string, templateId: string) => ["workspaces", ws, "agent-templates", templateId] as const,
};

/** The same-origin avatar route of a template (`has_avatar` on the card; the `Avatar` shows the initial when it fails). */
export function templateAvatarUrl(wsId: string, templateId: string): string {
  return `/v1/workspaces/${encodeURIComponent(wsId)}/agent-templates/${encodeURIComponent(templateId)}/avatar`;
}

/** The templates this workspace may use (C-1110), ordered by name; 404 `templates_disabled` reads as none. */
export function useWorkspaceTemplates(wsId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: workspaceTemplateKeys.list(wsId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/agent-templates", { params: { path: { ws: wsId! } } })),
    enabled: !!wsId && enabled,
    retry: false,
  });
}

/** The full plan of one template (C-1110): the card's fields, the content and the per-entry choices. */
export function useTemplatePlan(wsId: string | undefined, templateId: string | undefined) {
  return useQuery({
    queryKey: workspaceTemplateKeys.plan(wsId ?? "", templateId ?? ""),
    queryFn: () =>
      unwrap(api.GET("/v1/workspaces/{ws}/agent-templates/{template_id}", { params: { path: { ws: wsId!, template_id: templateId! } } })),
    enabled: !!wsId && !!templateId,
    retry: false,
  });
}

/** Create an agent from a template (C-1111); the caller refetches what it needs from the answer. */
export function useCreateAgentFromTemplate(wsId: string) {
  return useMutation({
    mutationFn: (body: CreateFromTemplateRequest) =>
      unwrap(api.POST("/v1/workspaces/{ws}/agents/from-template", { params: { path: { ws: wsId } }, body })),
  });
}

export const templateKeys = {
  sources: () => ["admin", "agent-template-sources"] as const,
  templates: () => ["admin", "agent-templates"] as const,
  version: (templateId: string, version: string) => ["admin", "agent-templates", templateId, version] as const,
  org: (org: string) => ["orgs", org, "agent-templates"] as const,
};

// ---- Administration: sources (C-1107) ---------------------------------------------------------

export function useTemplateSources() {
  return useQuery({
    queryKey: templateKeys.sources(),
    queryFn: () => unwrap(api.GET("/v1/admin/agent-template-sources")),
    retry: false,
  });
}

/** Check without saving (C-1107, as C-1124): one read of the repository, nothing stored. */
export function usePreviewTemplateSource() {
  return useMutation({
    mutationFn: (body: PreviewTemplateSourceRequest) => unwrap(api.POST("/v1/admin/agent-template-sources/preview", { body })),
  });
}

export function useCreateTemplateSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateTemplateSourceRequest) => unwrap(api.POST("/v1/admin/agent-template-sources", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: templateKeys.sources() }),
  });
}

export function useUpdateTemplateSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateTemplateSourceRequest }) =>
      unwrap(api.PATCH("/v1/admin/agent-template-sources/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: templateKeys.sources() }),
  });
}

/** Check now (C-1107): a read failure is a result, not an error. */
export function useCheckTemplateSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/admin/agent-template-sources/{id}/check", { params: { path: { id } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: templateKeys.sources() });
      void qc.invalidateQueries({ queryKey: templateKeys.templates() });
    },
  });
}

export function useDeleteTemplateSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/admin/agent-template-sources/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: templateKeys.sources() });
      void qc.invalidateQueries({ queryKey: templateKeys.templates() });
    },
  });
}

// ---- Administration: templates, review, approve (C-1108) --------------------------------------

export function useAdminTemplates() {
  return useQuery({
    queryKey: templateKeys.templates(),
    queryFn: () => unwrap(api.GET("/v1/admin/agent-templates")),
    retry: false,
  });
}

export function useTemplateVersion(templateId: string | undefined, version: string | undefined) {
  return useQuery({
    queryKey: templateKeys.version(templateId ?? "", version ?? ""),
    queryFn: () => unwrap(api.GET("/v1/admin/agent-templates/{template_id}/versions/{version}", { params: { path: { template_id: templateId!, version: version! } } })),
    enabled: !!templateId && !!version,
    retry: false,
  });
}

/** Approve exactly the content that was reviewed: the hash the page showed (C-1108). */
export function useApproveTemplateVersion(templateId: string, version: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (contentSha256: string) =>
      unwrap(
        api.POST("/v1/admin/agent-templates/{template_id}/versions/{version}/approve", {
          params: { path: { template_id: templateId, version } },
          body: { content_sha256: contentSha256 },
        }),
      ),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: templateKeys.templates() });
      void qc.invalidateQueries({ queryKey: templateKeys.version(templateId, version) });
    },
  });
}

export function useRejectTemplateVersion(templateId: string, version: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () =>
      unwrap(api.POST("/v1/admin/agent-templates/{template_id}/versions/{version}/reject", { params: { path: { template_id: templateId, version } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: templateKeys.templates() });
      void qc.invalidateQueries({ queryKey: templateKeys.version(templateId, version) });
    },
  });
}

/** Withdraw (false) or offer again (true); people keep using the current version while it is offered. */
export function useUpdateTemplate() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ templateId, body }: { templateId: string; body: UpdateTemplateRequest }) => {
      // Build the options object first: openapi-fetch's typing loses `body` on an
      // inline literal here (TS2353); `as const` keeps the literal's exact shape.
      const options = { params: { path: { template_id: templateId } }, body } as const;
      return unwrap(api.PATCH("/v1/admin/agent-templates/{template_id}", options));
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: templateKeys.templates() }),
  });
}

// ---- Organization switches (C-1109) -----------------------------------------------------------

export function useOrgTemplates(org: string | undefined) {
  return useQuery({
    queryKey: templateKeys.org(org ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/agent-templates", { params: { path: { org: org! } } })),
    enabled: !!org,
    retry: false,
  });
}

export function useSetOrgTemplate(org: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ templateId, enabled }: { templateId: string; enabled: boolean }) =>
      unwrap(api.PUT("/v1/orgs/{org}/agent-templates/{template_id}", { params: { path: { org, template_id: templateId } }, body: { enabled } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: templateKeys.org(org) }),
  });
}

// ---- The agent's provenance, update offer and helper files (C-1112, C-1143, C-1117) -----------

export type AgentTemplateStatus = components["schemas"]["AgentTemplateStatus"];
export type AgentTemplateRefView = components["schemas"]["AgentTemplateRefView"];
export type TemplateUpdateOffer = components["schemas"]["TemplateUpdateOffer"];
export type OfferPart = components["schemas"]["OfferPart"];
export type ApplyTemplateRequest = components["schemas"]["ApplyTemplateRequest"];
export type InputNeededView = components["schemas"]["InputNeededView"];
export type UpdateAppEntry = components["schemas"]["UpdateAppEntry"];
export type HelperFile = components["schemas"]["HelperFile"];

export const agentTemplateKeys = {
  status: (agentId: string) => ["agents", agentId, "template"] as const,
  helpers: (agentId: string) => ["agents", agentId, "helpers"] as const,
  helper: (agentId: string, path: string) => ["agents", agentId, "helpers", "content", path] as const,
};

/** The provenance and the update offer of one agent (C-1112). */
export function useAgentTemplate(agentId: string | undefined) {
  return useQuery({
    queryKey: agentTemplateKeys.status(agentId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/template", { params: { path: { agent: agentId! } } })),
    enabled: !!agentId,
    retry: false,
  });
}

/** Apply (or skip) an offered version (C-1112): only the listed parts change; the answer is the new status. */
export function useApplyAgentTemplate(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: ApplyTemplateRequest) =>
      unwrap(api.POST("/v1/agents/{agent}/template/apply", { params: { path: { agent: agentId } }, body })),
    onSuccess: (data) => {
      qc.setQueryData(agentTemplateKeys.status(agentId), data);
      void qc.invalidateQueries({ queryKey: agentTemplateKeys.helpers(agentId) });
    },
  });
}

/** Stop following this template (C-1112): the helper files and library skills stay. */
export function useDetachAgentTemplate(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.DELETE("/v1/agents/{agent}/template", { params: { path: { agent: agentId } } })),
    onSuccess: () => void qc.invalidateQueries({ queryKey: agentTemplateKeys.status(agentId) }),
  });
}

/** The agent's helper files (C-1143): path, size, executable — the content comes from `useAgentHelper`. */
export function useAgentHelpers(agentId: string | undefined) {
  return useQuery({
    queryKey: agentTemplateKeys.helpers(agentId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/helpers", { params: { path: { agent: agentId! } } })),
    enabled: !!agentId,
    retry: false,
  });
}

/** One helper file's bytes. Every helper file is UTF-8 text (owner decision 2026-11-24), so it reads as text. */
export async function getAgentHelperText(agentId: string, path: string): Promise<string> {
  const { data, error, response } = await api.GET("/v1/agents/{agent}/helpers/content", {
    params: { path: { agent: agentId }, query: { path } },
    parseAs: "text",
  });
  if (error !== undefined || !response.ok) {
    throw ApiError.fromBody(error, response.status);
  }
  return typeof data === "string" ? data : "";
}

/** The text of one helper file (C-1143). Every helper file is UTF-8 text (owner decision 2026-11-24), so it reads as text. */
export function useAgentHelperText(agentId: string, path: string) {
  return useQuery({
    queryKey: agentTemplateKeys.helper(agentId, path),
    queryFn: () => getAgentHelperText(agentId, path),
    retry: false,
    staleTime: Infinity,
  });
}

/** Remove one helper file (with `path`) or all of them (C-1143). */
export function useDeleteAgentHelpers(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (path: string | null) =>
      unwrap(api.DELETE("/v1/agents/{agent}/helpers", { params: { path: { agent: agentId }, query: path ? { path } : {} } })),
    onSuccess: () => void qc.invalidateQueries({ queryKey: agentTemplateKeys.helpers(agentId) }),
  });
}

