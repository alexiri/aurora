"use client";

// SSH access (P4-SSH-1, D-239, C-790..C-792): the person's registered ed25519 public keys, the certificate mint and the
// connection facts. Key writes need the dashboard session (an API key cannot); every route is 404 `ssh_disabled` while
// the instance's `features.ssh` is off, so callers only enable these queries when it is on.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError, api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type SshKey = S["SshKey"];
export type SshFacts = S["SshFacts"];
export type SshCertificate = S["SshCertificate"];

export type SshSession = S["SshSession"];
export type TerminatorInfo = S["TerminatorInfo"];

export const sshKeys = {
  list: ["me", "ssh-keys"] as const,
  facts: ["ssh", "facts"] as const,
};

/** (P4-SSH-2, C-793) Open SSH sessions: the caller's, one agent's, and the terminator replicas (instance admin). */
export const sshSessions = {
  mine: ["me", "ssh-sessions"] as const,
  agent: (agent: string) => ["agents", agent, "ssh-sessions"] as const,
  terminators: ["admin", "terminators"] as const,
};

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) && count < 1;

/** The caller's keys, newest first. */
export function useSshKeys(enabled = true) {
  return useQuery({
    queryKey: sshKeys.list,
    queryFn: async () => (await unwrap(api.GET("/v1/me/ssh-keys"))).keys,
    enabled,
    retry,
  });
}

export function useAddSshKey() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: S["CreateSshKeyRequest"]) => unwrap(api.POST("/v1/me/ssh-keys", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: sshKeys.list }),
  });
}

export function useRenameSshKey() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      unwrap(api.PATCH("/v1/me/ssh-keys/{id}", { params: { path: { id } }, body: { name } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: sshKeys.list }),
  });
}

export function useDeleteSshKey() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/me/ssh-keys/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: sshKeys.list }),
  });
}

/** Address, certificate lifetime and the terminators' host keys (`terminators_live` is 0 until one registers). */
export function useSshFacts(enabled = true) {
  return useQuery({
    queryKey: sshKeys.facts,
    queryFn: () => unwrap(api.GET("/v1/ssh")),
    enabled,
    retry,
    // A terminator may register while the page is open.
    refetchInterval: 30_000,
  });
}

/** Mint a short-lived certificate for `agent` certifying the registered key `publicKey` (C-791). */
export function useMintSshCertificate(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (publicKey: string) =>
      unwrap(api.POST("/v1/agents/{agent}/ssh-certificate", { params: { path: { agent } }, body: { public_key: publicKey } })),
    // The mint stamps the key's `last_used_at`.
    onSuccess: () => qc.invalidateQueries({ queryKey: sshKeys.list }),
  });
}

// A session opens and closes within seconds of the person's `ssh`; a list that is open on screen follows it.
const SESSIONS_REFRESH_MS = 10_000;

/** The caller's open sessions on every agent (Account › SSH keys). */
export function useMySshSessions(enabled = true) {
  return useQuery({
    queryKey: sshSessions.mine,
    queryFn: async () => (await unwrap(api.GET("/v1/me/ssh-sessions"))).sessions,
    enabled,
    retry,
    refetchInterval: SESSIONS_REFRESH_MS,
  });
}

/** The open sessions on one agent: all of them for someone who can own it (with `user`), else the caller's own. */
export function useAgentSshSessions(agent: string, enabled = true) {
  return useQuery({
    queryKey: sshSessions.agent(agent),
    queryFn: async () => (await unwrap(api.GET("/v1/agents/{agent}/ssh-sessions", { params: { path: { agent } } }))).sessions,
    enabled,
    retry,
    refetchInterval: SESSIONS_REFRESH_MS,
  });
}

/** Ask for a session to end (202): the terminator closes it at its next heartbeat, so the lists are refetched on a short delay too. */
export function useDisconnectSshSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/ssh-sessions/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: sshSessions.mine });
      void qc.invalidateQueries({ queryKey: ["agents"] });
    },
  });
}

/** The terminator replicas the API has seen (Admin › Overview; instance admins only). */
export function useTerminators(enabled = true) {
  return useQuery({
    queryKey: sshSessions.terminators,
    queryFn: async () => (await unwrap(api.GET("/v1/admin/terminators"))).terminators,
    enabled,
    retry,
    refetchInterval: 30_000,
  });
}

/** Plain-language text for the codes of C-790/C-791 (inline under the field); `null` when the code is not one of them. */
export function sshErrorText(err: unknown): string | null {
  if (!(err instanceof ApiError)) return null;
  switch (err.code) {
    case "ssh_key_invalid":
      return "This is not an ed25519 public key. Paste the one line from ~/.ssh/id_ed25519.pub.";
    case "ssh_key_unsupported_type":
      return "Only ed25519 keys are accepted. Create one with ssh-keygen -t ed25519.";
    case "ssh_key_exists":
      return "You already added this key.";
    case "ssh_key_limit":
      return "You can add at most 10 keys. Delete one first.";
    case "ssh_key_not_registered":
      return "This key is not registered. Add it under Account › SSH keys first.";
    case "ssh_not_supported":
      return "SSH is available for hosted agents only.";
    case "ssh_disabled":
      return "SSH is not enabled on this Aurora instance.";
    default:
      return null;
  }
}

/** `Retry-After` of a 429 mint, in seconds (`ApiError.details` does not carry it, the message names the hour). */
export function nextHourAt(now: Date = new Date()): string {
  const t = new Date(now);
  t.setMinutes(0, 0, 0);
  t.setHours(t.getHours() + 1);
  return t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
