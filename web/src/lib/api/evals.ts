"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type EvalQuestion = S["EvalQuestion"];
export type CreateEvalQuestion = S["CreateEvalQuestion"];
export type PatchEvalQuestion = S["PatchEvalQuestion"];
export type EvalSettings = S["EvalSettings"];
export type PutEvalSettings = S["PutEvalSettings"];
export type EvalRun = S["EvalRun"];
export type EvalRunDetail = S["EvalRunDetail"];
export type EvalResult = S["EvalResult"];
export type EvalSchedule = S["EvalSchedule"];
export type CreateEvalSchedule = S["CreateEvalSchedule"];
export type PatchEvalSchedule = S["PatchEvalSchedule"];

/** Limits of `02 §Limits` the page states in words (the API enforces them). */
export const EVAL_QUESTIONS_PER_AGENT = 100;
export const EVAL_QUESTION_CHARS = 2000;
export const EVAL_EXPECTED_TEXT_CHARS = 1000;
export const EVAL_CRITERIA_CHARS = 1000;
export const EVAL_RUNS_PER_AGENT_DAY = 20;
export const EVAL_SCHEDULES_PER_AGENT = 5;

/** Expected answers and answers are never cached outside React Query (SEC-116): every key lives under `agents/<id>/evals`. */
export const evalKeys = {
  all: (agent: string) => ["agents", agent, "evals"] as const,
  questions: (agent: string) => ["agents", agent, "evals", "questions"] as const,
  settings: (agent: string) => ["agents", agent, "evals", "settings"] as const,
  fingerprint: (agent: string) => ["agents", agent, "evals", "fingerprint"] as const,
  runs: (agent: string) => ["agents", agent, "evals", "runs"] as const,
  run: (agent: string, id: string) => ["agents", agent, "evals", "run", id] as const,
  schedules: (agent: string) => ["agents", agent, "evals", "schedules"] as const,
};

export function useEvalQuestions(agent: string) {
  return useQuery({
    queryKey: evalKeys.questions(agent),
    queryFn: async () =>
      (await unwrap(api.GET("/v1/agents/{agent}/eval-questions", { params: { path: { agent } } }))).questions,
    retry: false,
  });
}

export function useCreateEvalQuestion(agent: string) {
  return useInvalidatingMutation((body: CreateEvalQuestion) =>
    unwrap(api.POST("/v1/agents/{agent}/eval-questions", { params: { path: { agent } }, body })), evalKeys.all(agent));
}

export function usePatchEvalQuestion(agent: string) {
  return useInvalidatingMutation(({ id, body }: { id: string; body: PatchEvalQuestion }) =>
    unwrap(api.PATCH("/v1/eval-questions/{id}", { params: { path: { id } }, body })), evalKeys.all(agent));
}

export function useDeleteEvalQuestion(agent: string) {
  return useInvalidatingMutation((id: string) =>
    unwrap(api.DELETE("/v1/eval-questions/{id}", { params: { path: { id } } })), evalKeys.all(agent));
}

export function useEvalSettings(agent: string) {
  return useQuery({
    queryKey: evalKeys.settings(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/eval-settings", { params: { path: { agent } } })),
    retry: false,
  });
}

export function usePutEvalSettings(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PutEvalSettings) =>
      unwrap(api.PUT("/v1/agents/{agent}/eval-settings", { params: { path: { agent } }, body })),
    onSuccess: (data) => {
      qc.setQueryData(evalKeys.settings(agent), data);
      void qc.invalidateQueries({ queryKey: evalKeys.fingerprint(agent) });
    },
  });
}

export function useEvalRuns(agent: string) {
  return useInfiniteQuery({
    queryKey: evalKeys.runs(agent),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/agents/{agent}/eval-runs", {
          params: { path: { agent }, query: { cursor: pageParam, limit: 20 } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // A queued or running run is followed; a quiet list is not polled.
    refetchInterval: (q) =>
      q.state.data?.pages.some((p) => p.items.some((r) => r.status === "queued" || r.status === "running")) ? 5000 : false,
    retry: false,
  });
}

export function useEvalRun(agent: string, id: string | null) {
  return useQuery({
    queryKey: evalKeys.run(agent, id ?? ""),
    enabled: !!id,
    queryFn: () => unwrap(api.GET("/v1/eval-runs/{id}", { params: { path: { id: id! } } })),
    refetchInterval: (q) => (q.state.data?.status === "queued" || q.state.data?.status === "running" ? 5000 : false),
    retry: false,
  });
}

export function useStartEvalRun(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.POST("/v1/agents/{agent}/eval-runs", { params: { path: { agent } } })),
    onSettled: () => qc.invalidateQueries({ queryKey: evalKeys.runs(agent) }),
  });
}

export function useCancelEvalRun(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/eval-runs/{id}/cancel", { params: { path: { id } } })),
    onSettled: () => qc.invalidateQueries({ queryKey: evalKeys.all(agent) }),
  });
}

export function useDeleteEvalRun(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/eval-runs/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: evalKeys.all(agent) }),
  });
}

export function useEvalSchedules(agent: string) {
  return useQuery({
    queryKey: evalKeys.schedules(agent),
    queryFn: async () =>
      (await unwrap(api.GET("/v1/agents/{agent}/eval-schedules", { params: { path: { agent } } }))).schedules,
    retry: false,
  });
}

export function useCreateEvalSchedule(agent: string) {
  return useInvalidatingMutation((body: CreateEvalSchedule) =>
    unwrap(api.POST("/v1/agents/{agent}/eval-schedules", { params: { path: { agent } }, body })), evalKeys.schedules(agent));
}

export function usePatchEvalSchedule(agent: string) {
  return useInvalidatingMutation(({ id, body }: { id: string; body: PatchEvalSchedule }) =>
    unwrap(api.PATCH("/v1/eval-schedules/{id}", { params: { path: { id } }, body })), evalKeys.schedules(agent));
}

export function useDeleteEvalSchedule(agent: string) {
  return useInvalidatingMutation((id: string) => unwrap(api.DELETE("/v1/eval-schedules/{id}", { params: { path: { id } } })), evalKeys.schedules(agent));
}

export function useEvalFingerprint(agent: string, enabled = true) {
  return useQuery({
    queryKey: evalKeys.fingerprint(agent),
    enabled,
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/eval-fingerprint", { params: { path: { agent } } })),
    retry: false,
  });
}
