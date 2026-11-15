"use client";

import Link from "next/link";
import { useState } from "react";

import { AddConnectorFlow } from "@/components/connectors/add-connector";
import { Button, buttonVariants } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import type { ConnectorAvailability } from "@/lib/api/connectors";
import { cn } from "@/lib/utils";

/** Who to ask, in words ("Ana, Ben"), or a generic fallback. */
export function adminNames(admins: { name: string }[]): string {
  return admins.length ? admins.map((a) => a.name).join(", ") : "your instance administrator";
}

const HEAD: Record<string, (name: string) => string> = {
  not_added: (n) => `An instance administrator needs to add ${n} first.`,
  waiting_for_host: (n) => `${n} is added, but waiting for the connector host. The operator must start it.`,
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
  const head = (HEAD[item.setup_state] ?? HEAD.not_added!)(item.name);
  return (
    <li>
      <Notice tone="pending" role="status" className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <span className="min-w-0 flex-1 basis-60" data-testid={`blocked-${item.type_id}`}>
          <strong className="font-semibold">{head}</strong>{" "}
          {isInstanceAdmin ? "You are an instance administrator." : <>Ask: {adminNames(admins)}.</>}
        </span>
        {isInstanceAdmin ? (
          item.setup_state === "not_added" ? (
            <Button size="sm" onClick={() => setAdding(true)}>
              Add {item.name}
            </Button>
          ) : (
            <Link href="/admin/connectors" className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
              Open Admin › Connectors
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
        <Link href="/admin/connectors" className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
          Open Admin › Connectors
        </Link>
      ) : null}
    </Notice>
  );
}
