"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";

import { CreateOrgForm } from "@/components/create-org-form";
import { Button, buttonVariants } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { SearchIcon } from "@/components/ui/icons";
import { useCreateOrg, useInstance, useMe } from "@/lib/api/queries";
import { lastScope } from "@/lib/nav/recent-scopes";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { nameInSentence } from "@/lib/workspace-name";

export type NotFoundKind = "organization" | "workspace" | "page";

/**
 * (P5-REFINE-2, org-3) "Not found" inside the app shell: a dashed card with a search icon, a title, one sentence and two
 * buttons. An unknown organization (or workspace) says what was looked for and offers **Go to <your last or only
 * organization>** (primary) and **Create an organization** (outline, only where creating one is allowed); any other
 * unknown path says "That page doesn't exist" with **Go home**. The shell around it shows the viewer's real organization,
 * never the unknown slug.
 */
export function NotFoundState({ kind = "page", slug }: { kind?: NotFoundKind; slug?: string }) {
  const me = useMe();
  const instance = useInstance();
  const router = useRouter();
  const [creating, setCreating] = useState(false);
  const create = useCreateOrg();
  const last = lastScope(me.data?.user.id, me.data?.memberships);
  const canCreate = me.data?.user.is_instance_admin === true || instance.data?.org_creation === "anyone";
  const home = last ? (last.wsSlug ? wsRoutes(last.orgSlug, last.wsSlug).base : orgRoutes(last.orgSlug).home()) : "/";

  const title =
    kind === "organization" ? "We couldn't find that organization" : kind === "workspace" ? "We couldn't find that workspace" : "That page doesn't exist";
  const text =
    kind === "page"
      ? "Check the address, or go back to where you were."
      : kind === "workspace"
        ? `There is no workspace called “${slug ?? ""}” here, or you don't have access to it. Check the address, or pick one of yours.`
        : `There is no organization called “${slug ?? ""}”, or you don't have access to it. Check the address, or pick one of yours.`;

  return (
    <div data-testid="not-found" className="mx-auto max-w-xl rounded-card border border-dashed border-line-strong bg-surface p-6 sm:p-8">
      <span aria-hidden="true" className="flex size-10 items-center justify-center rounded-control bg-subtle text-ink-2 [&_svg]:size-5">
        <SearchIcon />
      </span>
      <h1 className="mt-4 font-display text-lg font-semibold text-ink">{title}</h1>
      <p className="mt-2 text-sm text-ink-2">{text}</p>
      <div className="mt-5 flex flex-wrap gap-2">
        <Link href={home} className={buttonVariants({ variant: "primary" })}>
          {kind === "page" ? "Go home" : last ? `Go to ${nameInSentence(last.wsName && kind === "workspace" ? last.wsName : last.orgName)}` : "Go home"}
        </Link>
        {kind !== "page" && canCreate ? (
          <Button type="button" variant="outline" onClick={() => setCreating(true)}>
            Create an organization
          </Button>
        ) : null}
      </div>
      {kind !== "page" && canCreate ? (
        <Dialog open={creating} onOpenChange={setCreating}>
          <DialogContent title="Create an organization" description="You become its owner.">
            <CreateOrgForm
              pending={create.isPending}
              error={create.error}
              onSubmit={(b) => create.mutate(b, { onSuccess: (r) => router.replace(`/o/${r.org.slug}/w/${r.workspace.slug}`) })}
            />
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}
