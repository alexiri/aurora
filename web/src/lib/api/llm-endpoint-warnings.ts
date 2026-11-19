"use client";

import { useQuery } from "@tanstack/react-query";

import { api } from "./client";
import { retryUnlessClientError } from "./errors";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type EndpointWarnings = components["schemas"]["EndpointWarnings"];

/**
 * (P5-POLISH-3, C-1034, D-385) The endpoints whose model roles are unset or not allowed, one entry per endpoint with at
 * least one warning. Instance admins only: Administration › AI models shows each once on the endpoint's row.
 */
export function useLlmEndpointWarnings(enabled = true) {
  return useQuery({
    // Under the settings key, so saving any settings page refetches it.
    queryKey: ["admin", "settings", "llm-endpoint-warnings"] as const,
    queryFn: () => unwrap(api.GET("/v1/admin/llm-endpoint-warnings")),
    enabled,
    retry: retryUnlessClientError,
  });
}
