"use client";

import Link from "next/link";
import { useState } from "react";

import { Avatar } from "@/components/ui/avatar";
import { Button, buttonVariants } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { type AppSetupRequest, useDeclineSetup } from "@/lib/api/app-setup-requests";
import { cn } from "@/lib/utils";

/** "Bob wants", "Bob and Carol want", "Bob, Carol and 2 others want" (SEC-135: display names only). */
export function askersPhrase(names: string[]): { who: string; verb: "wants" | "want" } {
  if (names.length <= 1) return { who: names[0] ?? "Someone", verb: "wants" };
  if (names.length === 2) return { who: `${names[0]} and ${names[1]}`, verb: "want" };
  const others = names.length - 2;
  return { who: `${names[0]}, ${names[1]} and ${others} ${others === 1 ? "other" : "others"}`, verb: "want" };
}

/** One line on what is missing, in plain words. */
export function needSentence(r: Pick<AppSetupRequest, "app_name" | "needs" | "unavailable">): string {
  if (r.unavailable) return `${r.app_name} is switched off for your organization.`;
  if (r.needs?.includes("oauth_client")) return `A sign-in app for ${r.app_name} needs to be registered once.`;
  if (r.needs?.includes("instance")) return `A ${r.app_name} server needs to be registered.`;
  return `${r.app_name} needs set-up only an owner or admin can do.`;
}

/**
 * (P5-POLISH-4, D-386, C-1030) A request for an app, in the Overview's Needs you: who asked, what is missing,
 * **Set up** (to the organization's Apps tab, at the app or at the sign-in app form) and **Not now**.
 */
export function SetupRequestItem({ request, orgBase, orgId }: { request: AppSetupRequest; orgBase: string; orgId: string }) {
  const decline = useDeclineSetup(orgId);
  const [open, setOpen] = useState(false);
  const names = request.askers.map((a) => a.display_name);
  const { who, verb } = askersPhrase(names);
  const tab = !request.unavailable && request.needs?.includes("oauth_client") ? "oauth-clients" : "apps";
  const href = `${orgBase}/apps?tab=${tab}&app=${encodeURIComponent(request.app_id)}`;
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-control bg-surface px-3 py-2 text-sm">
      <Avatar name={names[0] ?? "?"} tintKey={request.askers[0]?.user_id ?? request.id} size={28} />
      <span className="min-w-0 flex-1">
        <span className="block font-medium">
          {who} {verb} {request.app_name}
        </span>
        <span className="block text-xs text-muted">{needSentence(request)}</span>
      </span>
      <span className="flex shrink-0 items-center gap-2">
        <Link href={href} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          Set up
        </Link>
        <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>
          Not now
        </Button>
      </span>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`Not setting up ${request.app_name} now?`}
        description={`${names.length === 1 ? names[0] : `${names.length} people`} will be told you aren't setting up ${request.app_name} now.`}
        confirmLabel="Not now"
        busy={decline.isPending}
        onConfirm={() => decline.mutate(request.id, { onSuccess: () => setOpen(false) })}
      />
    </li>
  );
}
