"use client";

import { ErrorState, Loading } from "@/components/states";
import { Section } from "@/components/ui/section";
import { useSession } from "@/lib/api/queries";

import { ApprovalNotifications } from "./approval-notifications";

/** Account › Notifications (account-4): how you hear about approvals, in this browser and on your devices. */
export function NotificationsView() {
  const me = useSession();
  if (me.isPending) return <Loading />;
  if (me.isError) return <ErrorState error={me.error} onRetry={() => me.refetch()} />;
  return (
    <Section id="notifications" title="Approval notifications" description="How you hear about approvals that need a decision." card>
      <ApprovalNotifications me={me.data} />
    </Section>
  );
}
