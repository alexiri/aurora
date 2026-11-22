"use client";

// (FIX-218, D-459, C-1145) The person's shell choice for one agent, stored on the server so it follows them across browsers
// and into SSH: `GET`/`PUT /v1/agents/{agent}/terminal/shell`. The web terminal no longer names the shell itself: the server
// applies this value when `?shell=` is absent.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { retryUnlessClientError } from "./errors";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type TerminalShellPreference = components["schemas"]["TerminalShellPreference"];

export const terminalShellKey = (agentId: string) => ["agents", agentId, "terminal-shell"] as const;

export function useTerminalShell(agentId: string) {
  return useQuery({
    queryKey: terminalShellKey(agentId),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/terminal/shell", { params: { path: { agent: agentId } } })),
    retry: retryUnlessClientError,
  });
}

export function useSaveTerminalShell(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (shell: "bash" | "zsh") =>
      unwrap(api.PUT("/v1/agents/{agent}/terminal/shell", { params: { path: { agent: agentId } }, body: { shell } })),
    onSuccess: (pref) => qc.setQueryData(terminalShellKey(agentId), pref),
  });
}
