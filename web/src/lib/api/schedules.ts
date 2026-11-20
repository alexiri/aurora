"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Schedule = S["Schedule"];
export type ScheduleRun = S["ScheduleRun"];
export type CreateScheduleRequest = S["CreateScheduleRequest"];
export type UpdateScheduleRequest = S["UpdateScheduleRequest"];
/** (P5-REFINE-7, C-1077) What a cadence would do: three instants, the smallest gap, too frequent. */
export type SchedulePreview = S["SchedulePreview"];

export const scheduleKeys = {
  list: (agent: string) => ["agents", agent, "schedules"] as const,
  runs: (agent: string, id: string) => ["agents", agent, "schedules", id, "runs"] as const,
};

export function useSchedules(agent: string, enabled = true) {
  return useQuery({
    queryKey: scheduleKeys.list(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/schedules", { params: { path: { agent } } })),
    enabled,
    retry: false,
  });
}

export function useScheduleRuns(agent: string, id: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: scheduleKeys.runs(agent, id),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/schedules/{id}/runs", {
          params: { path: { id }, query: { cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled,
    retry: false,
  });
}

export function useCreateSchedule(agent: string) {
  return useInvalidatingMutation((body: CreateScheduleRequest) =>
    unwrap(api.POST("/v1/agents/{agent}/schedules", { params: { path: { agent } }, body })), scheduleKeys.list(agent));
}

export function useUpdateSchedule(agent: string) {
  return useInvalidatingMutation(({ id, body }: { id: string; body: UpdateScheduleRequest }) =>
    unwrap(api.PATCH("/v1/schedules/{id}", { params: { path: { id } }, body })), scheduleKeys.list(agent));
}

export function useDeleteSchedule(agent: string) {
  return useInvalidatingMutation((id: string) => unwrap(api.DELETE("/v1/schedules/{id}", { params: { path: { id } } })), scheduleKeys.list(agent));
}

export function useRunScheduleNow(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/schedules/{id}/run-now", { params: { path: { id } } })),
    onSettled: (_d, _e, id) => {
      void qc.invalidateQueries({ queryKey: scheduleKeys.runs(agent, id) });
      void qc.invalidateQueries({ queryKey: scheduleKeys.list(agent) });
    },
  });
}

/**
 * (P5-REFINE-7, C-1077) Preview a cadence as you type (rate class `check`: debounce the input). Saves nothing;
 * a bad cron or time zone answers 422 `invalid_cron` / `invalid_timezone`. Disabled until `cron` has five fields.
 */
export function useSchedulePreview(agent: string, cron: string, timezone?: string, runAt = "") {
  const trimmed = cron.trim();
  return useQuery({
    queryKey: ["agents", agent, "schedules", "preview", trimmed, runAt, timezone ?? ""] as const,
    queryFn: () =>
      unwrap(
        api.POST("/v1/agents/{agent}/schedules/preview", {
          params: { path: { agent } },
          // (P5-SCHED-ONCE, C-1092) A one-time schedule previews its `run_at` instead of a cron.
          body: { ...(runAt ? { run_at: runAt } : { cron: trimmed }), ...(timezone ? { timezone } : {}) },
        }),
      ),
    enabled: runAt ? true : cronFieldCount(trimmed) === 5,
    retry: false,
    staleTime: 60_000,
  });
}

/** Number of whitespace-separated fields; the server is authoritative for the rest of cron validation. */
export function cronFieldCount(cron: string): number {
  return cron.trim().split(/\s+/).filter(Boolean).length;
}
