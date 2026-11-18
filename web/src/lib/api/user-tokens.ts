"use client";

// Send as me (phase 5, P4-UTOK, D-281..D-284, C-853..C-856): the person's own chat-platform tokens, their grants to
// single agents, and the posts agents propose to make as them. The token value is sent once and never comes back;
// every write needs the dashboard session.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, api } from "./client";
import { retryUnlessClientError } from "./errors";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type UserToken = S["UserToken"];
export type OwnSendApproval = S["OwnSendApproval"];

export const userTokenKeys = {
  list: ["me", "user-tokens"] as const,
  sends: ["me", "user-token-approvals"] as const,
};

/** Poll while the page is open: an agent may propose a post at any moment. */
export const OWN_SEND_POLL_MS = 10_000;

/** The caller's own tokens (never the value). */
export function useUserTokens() {
  return useQuery({
    queryKey: userTokenKeys.list,
    queryFn: async () => (await unwrap(api.GET("/v1/me/user-tokens"))).tokens,
    retry: retryUnlessClientError,
  });
}

/** Adds a token: sealed and verified against your own chat account before it is stored. */
export function useAddUserToken() {
  return useInvalidatingMutation((body: S["CreateUserTokenRequest"]) => unwrap(api.POST("/v1/me/user-tokens", { body })), userTokenKeys.list);
}

/** Revokes a token: agents stop posting as you and pending posts are cancelled. */
export function useRevokeUserToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/me/user-tokens/{id}", { params: { path: { id } } })),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: userTokenKeys.list });
      void qc.invalidateQueries({ queryKey: userTokenKeys.sends });
    },
  });
}

/** Grants (`granted: true`) or removes one agent's use of a token. */
export function useSetUserTokenGrant() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, agent, granted }: { id: string; agent: string; granted: boolean }) =>
      granted
        ? unwrap(api.PUT("/v1/me/user-tokens/{id}/agents/{agent}", { params: { path: { id, agent } } }))
        : unwrap(api.DELETE("/v1/me/user-tokens/{id}/agents/{agent}", { params: { path: { id, agent } } })),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: userTokenKeys.list });
      void qc.invalidateQueries({ queryKey: userTokenKeys.sends });
    },
  });
}

/** The posts agents proposed to make as you, with their full text (yours only). */
export function useOwnSendApprovals(status?: string) {
  return useQuery({
    queryKey: [...userTokenKeys.sends, status ?? "all"] as const,
    queryFn: async () =>
      (await unwrap(api.GET("/v1/me/user-token-approvals", { params: { query: { status } } }))).items,
    refetchInterval: OWN_SEND_POLL_MS,
    retry: retryUnlessClientError,
  });
}

/** Decides one of your own proposed posts; nobody else can. */
export function useDecideOwnSend() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, decision }: { id: string; decision: "approve" | "deny" }) =>
      unwrap(api.POST("/v1/action-approvals/{id}/decision", { params: { path: { id } }, body: { decision } })),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: userTokenKeys.sends });
      void qc.invalidateQueries({ queryKey: ["action-approvals"] });
    },
  });
}

/** Plain-language text for the refusals of C-853 (inline under the field); `null` for any other error. */
export function userTokenErrorText(err: unknown): string | null {
  if (!(err instanceof ApiError)) return null;
  switch (err.code) {
    case "user_token_mismatch":
      return "This token belongs to another account. Create it in the chat account you linked here.";
    case "user_token_invalid":
      return "The chat platform rejected this token. Check that you copied all of it and that it is still active.";
    case "user_token_limit":
      return "You can keep at most 5 tokens. Revoke one first.";
    case "user_token_exists":
      return "You already have a token for this chat account. Revoke it first to add a new one.";
    case "user_link_required":
      return "Link this chat account to Aurora first, then add a token.";
    case "user_tokens_unsupported":
      return "This chat platform cannot post as a person.";
    case "secret_store_unavailable":
      return "Aurora cannot store secrets right now, so nothing was saved. Try again in a moment.";
    case "connector_unavailable":
      return "The chat platform did not answer in time. Try again in a moment.";
    default:
      return null;
  }
}
