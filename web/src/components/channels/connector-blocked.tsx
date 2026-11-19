"use client";

import Link from "next/link";
import { useState } from "react";

import { AddConnectorFlow } from "@/components/connectors/add-connector";
import { Button, buttonVariants } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { Unavailable } from "@/components/ui/unavailable";
import type { ConnectorAvailability } from "@/lib/api/connectors";
import { adminRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/** Who to ask, in words ("Ana, Ben"), or a generic fallback. */
export function adminNames(admins: { name: string }[]): string {
  return admins.length ? admins.map((a) => a.name).join(", ") : "your instance administrator";
}

/** The platform's name for people: never the connector's raw id ("fake" → "Fake"). */
export function platformName(item: { name: string; type_id: string }): string {
  const n = item.name.trim();
  if (n === "") return item.type_id;
  return n === item.type_id ? n.charAt(0).toUpperCase() + n.slice(1) : n;
}

const HEAD: Record<string, (name: string) => string> = {
  not_added: (n) => `An instance administrator needs to add ${n} first.`,
  disabled: (n) => `${n} is disabled. An instance administrator can enable it.`,
  needs_approval: (n) => `${n} needs an instance administrator's approval before it can be used.`,
};

/**
 * (FIX-127, C-712) A connector type that is not usable yet: says why, and who acts next. For a
 * viewer who is an instance admin it links straight to the fix (or, when the type is not added,
 * adds it here in a dialog); everyone else sees the administrators' names.
 */
export function ConnectorBlocked({
  item,
  admins,
  isInstanceAdmin,
}: {
  item: ConnectorAvailability;
  admins: { name: string }[];
  isInstanceAdmin: boolean;
}) {
  const [adding, setAdding] = useState(false);
  const name = platformName(item);
  if (item.setup_state === "waiting_for_host") {
    // (P5-POLISH-10, E6) One neutral card, not an amber notice: nothing here needs the viewer unless they run the host.
    return (
      <li data-testid={`blocked-${item.type_id}`}>
        <Unavailable
          title={`${name} is added, but its connector isn't running`}
          fixer={isInstanceAdmin ? "Start the connector host that runs it." : <>Ask: {adminNames(admins)}.</>}
          guide={
            isInstanceAdmin ? (
              <Link href={adminRoutes.chatPlatforms()} className="font-medium text-link underline">
                Open Administration › Chat platforms
              </Link>
            ) : undefined
          }>
          Until an operator starts it, {name} can&apos;t be connected.
        </Unavailable>
      </li>
    );
  }
  const head = (HEAD[item.setup_state] ?? HEAD.not_added!)(name);
  if (!isInstanceAdmin) {
    // (P5-POLISH-11, F1) Nothing here the viewer can act on (there is no way to ask for a platform): a neutral card, not an amber notice.
    return (
      <li data-testid={`blocked-${item.type_id}`}>
        <Unavailable title={head} fixer={<>Ask: {adminNames(admins)}.</>}>
          Until then, {name} can&apos;t be connected.
        </Unavailable>
      </li>
    );
  }
  return (
    <li>
      <Notice tone="pending" role="status" className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <span className="min-w-0 flex-1 basis-60" data-testid={`blocked-${item.type_id}`}>
          <strong className="font-semibold">{head}</strong> {isInstanceAdmin ? null : <>Ask: {adminNames(admins)}.</>}
        </span>
        {isInstanceAdmin ? (
          item.setup_state === "not_added" ? (
            <Button size="sm" onClick={() => setAdding(true)}>
              Add {item.name}
            </Button>
          ) : (
            <Link href={adminRoutes.chatPlatforms()} className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
              Open Administration › Chat platforms
            </Link>
          )
        ) : null}
      </Notice>
      {adding ? (
        <Dialog open onOpenChange={(o) => !o && setAdding(false)}>
          <DialogContent title={`Add ${item.name}`} description="Adding approves the connector and makes it available to every organization.">
            <AddConnectorFlow only={item.type_id} onAdded={() => setAdding(false)} />
          </DialogContent>
        </Dialog>
      ) : null}
    </li>
  );
}

/** Nothing at all is announced: the connector host is not running. */
export function NoConnectorsYet({ admins, isInstanceAdmin }: { admins: { name: string }[]; isInstanceAdmin: boolean }) {
  return (
    <Notice tone="pending" role="status" className="flex flex-wrap items-center gap-x-4 gap-y-2">
      <span className="min-w-0 flex-1 basis-60">
        <strong className="font-semibold">No chat platform is available yet.</strong>{" "}
        {isInstanceAdmin ? "Start the connector host, then add a connector." : <>Ask {adminNames(admins)} to set one up.</>}
      </span>
      {isInstanceAdmin ? (
        <Link href={adminRoutes.chatPlatforms()} className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
          Open Administration › Chat platforms
        </Link>
      ) : null}
    </Notice>
  );
}
