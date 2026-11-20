"use client";

import Link from "next/link";
import { useState } from "react";

import { DeclineRequestDialog } from "@/components/attention/decline-dialog";
import { Avatar } from "@/components/ui/avatar";
import { Button, buttonVariants } from "@/components/ui/button";
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
export function needSentence(r: Pick<AppSetupRequest, "app_name" | "needs" | "unavailable"> & { subject?: string }): string {
  if (r.subject === "chat_platform") return "Your organization has no chat platform connected for this.";
  if (r.subject === "ai_key") return "Your organization has no AI key its agents can use.";
  if (r.unavailable) return `${r.app_name} is switched off for your organization.`;
  if (r.needs?.includes("oauth_client")) return `A sign-in app for ${r.app_name} needs to be registered once.`;
  if (r.needs?.includes("instance")) return `A ${r.app_name} server needs to be registered.`;
  return `${r.app_name} needs set-up only an owner or admin can do.`;
}

/** **Decline** for one setup request wherever it shows (org Apps row, Needs you): the shared dialog with an optional message. */
export function SetupDeclineButton({ request, orgId, noun }: { request: AppSetupRequest; orgId: string; noun: string }) {
  const decline = useDeclineSetup(orgId);
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" variant="ghost" onClick={() => setOpen(true)}>
        Decline
      </Button>
      <DeclineRequestDialog
        open={open}
        onOpenChange={setOpen}
        names={request.askers.map((a) => a.display_name)}
        noun={noun}
        busy={decline.isPending}
        error={decline.error}
        onDecline={(message) => decline.mutate({ id: request.id, message }, { onSuccess: () => setOpen(false) })}
      />
    </>
  );
}

/**
 * (P5-POLISH-4, D-386, C-1030) A request for an app, in the Overview's Needs you: who asked, what is missing,
 * **Set up** (to the organization's Apps tab, at the app or at the sign-in app form) and **Decline** (a dialog with an
 * optional message the askers read, C-1081).
 */
export function SetupRequestItem({ request, orgBase, orgId }: { request: AppSetupRequest; orgBase: string; orgId: string }) {
  const decline = useDeclineSetup(orgId);
  const [open, setOpen] = useState(false);
  const names = request.askers.map((a) => a.display_name);
  const { who, verb } = askersPhrase(names);
  // (P5-APIGAP-2, C-1047) A chat platform or an AI key: the noun in a sentence, and the action that fixes it.
  const subject = request.subject;
  const noun =
    subject === "ai_key" ? "an AI key" : subject === "chat_platform" ? (request.connector_type ? request.app_name : "a chat platform") : request.app_name;
  const tab = !request.unavailable && request.needs?.includes("oauth_client") ? "oauth-clients" : "apps";
  const href =
    subject === "chat_platform"
      ? `${orgBase}/chat-platforms`
      : subject === "ai_key"
        ? `${orgBase}/apps?tab=accounts&kind=ai-key`
        : `${orgBase}/apps?tab=${tab}&app=${encodeURIComponent(request.app_id)}`;
  const action = subject === "chat_platform" ? "Connect a chat platform" : subject === "ai_key" ? "Add an AI key" : "Set up";
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-control bg-surface px-3 py-2 text-sm">
      <Avatar name={names[0] ?? "?"} tintKey={request.askers[0]?.user_id ?? request.id} size={28} />
      <span className="min-w-0 flex-1">
        <span className="block font-medium">
          {who} {verb} {noun}
        </span>
        <span className="block text-xs text-muted">{needSentence(request)}</span>
      </span>
      <span className="flex shrink-0 items-center gap-2">
        <Link href={href} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          {action}
        </Link>
        <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>
          Decline
        </Button>
      </span>
      <DeclineRequestDialog
        open={open}
        onOpenChange={setOpen}
        names={names}
        noun={noun}
        busy={decline.isPending}
        error={decline.error}
        onDecline={(message) => decline.mutate({ id: request.id, message }, { onSuccess: () => setOpen(false) })}
      />
    </li>
  );
}
