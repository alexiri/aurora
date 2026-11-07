"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { queryKeys } from "./queries";
import type { components } from "./schema";

export type AgentDto = components["schemas"]["Agent"];

/** Mirrors the backend limit (`AVATAR_MAX`); the server stays authoritative. */
export const AVATAR_MAX_BYTES = 512 * 1024;
export const AVATAR_TYPES = ["image/png", "image/jpeg", "image/webp"] as const;

/** Client-side pre-check for UX only; returns an error message or null. */
export function avatarProblem(file: Pick<File, "type" | "size">): string | null {
  if (!(AVATAR_TYPES as readonly string[]).includes(file.type)) return "Choose a PNG, JPEG or WebP image.";
  if (file.size > AVATAR_MAX_BYTES) return "That image is larger than 512 KiB.";
  return null;
}

export function avatarErrorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 413) return "That image is larger than 512 KiB.";
    if (err.code === "avatar_invalid") return err.message || "That image could not be used.";
    return err.message || "Avatar update failed.";
  }
  return "Avatar update failed. Check your connection and try again.";
}

/** Raw-body PUT (contract C-41): the bytes are the request body; the server sniffs the type. */
async function putAvatar(agentId: string, file: File): Promise<AgentDto> {
  const res = await fetch(`/v1/agents/${encodeURIComponent(agentId)}/avatar`, {
    method: "PUT",
    credentials: "same-origin",
    headers: { "content-type": "application/octet-stream" },
    body: file,
  });
  const body: unknown = await res.json().catch(() => null);
  if (!res.ok) throw ApiError.fromBody(body, res.status);
  return body as AgentDto;
}

export function useUploadAvatar(wsId: string | undefined, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (file: File) => putAvatar(agentId, file),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") }),
  });
}

export function useRemoveAvatar(wsId: string | undefined, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { error, response } = await api.DELETE("/v1/agents/{agent}/avatar", {
        params: { path: { agent: agentId } },
      });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") }),
  });
}
