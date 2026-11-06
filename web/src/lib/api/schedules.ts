"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Schedule = S["Schedule"];
export type ScheduleRun = S["ScheduleRun"];
export type CreateScheduleRequest = S["CreateScheduleRequest"];
export type UpdateScheduleRequest = S["UpdateScheduleRequest"];

export const scheduleKeys = {
  list: (agent: string) => ["agents", agent, "schedules"] as const,
  runs: (agent: string, id: string) => ["agents", agent, "schedules", id, "runs"] as const,
};

export function useSchedules(agent: string) {
  return useQuery({
    queryKey: scheduleKeys.list(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/schedules", { params: { path: { agent } } })),
    retry: false,
  });
}

export function useScheduleRuns(agent: string, id: string, enabled: boolean) {
  return useQuery({
    queryKey: scheduleKeys.runs(agent, id),
    queryFn: () => unwrap(api.GET("/v1/schedules/{id}/runs", { params: { path: { id } } })),
    enabled,
    retry: false,
  });
}

export function useCreateSchedule(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateScheduleRequest) =>
      unwrap(api.POST("/v1/agents/{agent}/schedules", { params: { path: { agent } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: scheduleKeys.list(agent) }),
  });
}

export function useUpdateSchedule(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateScheduleRequest }) =>
      unwrap(api.PATCH("/v1/schedules/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: scheduleKeys.list(agent) }),
  });
}

export function useDeleteSchedule(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/schedules/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: scheduleKeys.list(agent) }),
  });
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

/** Number of whitespace-separated fields; the server is authoritative for the rest of cron validation. */
export function cronFieldCount(cron: string): number {
  return cron.trim().split(/\s+/).filter(Boolean).length;
}
