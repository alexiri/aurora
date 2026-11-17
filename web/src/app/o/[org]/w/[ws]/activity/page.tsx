"use client";

import { useParams } from "next/navigation";

import { ActivityView } from "@/components/approvals/activity-view";
import { ErrorState, Loading } from "@/components/states";
import { TopicPage } from "@/components/topic-page";
import { UsageView } from "@/components/usage/usage-view";
import { useResolve } from "@/lib/api/queries";
import type { TABS } from "@/lib/routes";

type ActivityTab = (typeof TABS)["wsActivity"][number];

/** (P5-USAGE-2, C-1004) Activity has two tabs: Requests (the request log, the default) and Usage. */
export default function Page() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolved.data.workspace) return <ErrorState error={{ code: "not_found" }} />;
  const wsId = resolved.data.workspace.id;
  const base = `/o/${org}/w/${ws}`;
  return (
    <TopicPage<ActivityTab>
      idBase="activity"
      title="Activity"
      tabs={[
        { id: "requests", label: "Requests", content: <ActivityView wsId={wsId} base={base} /> },
        { id: "usage", label: "Usage", content: <UsageView level={{ kind: "workspace", wsId, base }} /> },
      ]}
    />
  );
}
