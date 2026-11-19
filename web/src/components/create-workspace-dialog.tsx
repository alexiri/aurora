"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { useCreateWorkspace } from "@/lib/api/queries";

import { CreateWorkspaceForm } from "./create-workspace-form";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogTrigger } from "./ui/dialog";

/** With `open`/`onOpenChange` the caller owns the state and supplies its own trigger (the rail's switcher does). */
export function CreateWorkspaceDialog({
  orgId,
  orgSlug,
  open: controlledOpen,
  onOpenChange,
}: {
  orgId: string;
  orgSlug: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [own, setOwn] = useState(false);
  const controlled = controlledOpen !== undefined;
  const open = controlled ? controlledOpen : own;
  const setOpen = (o: boolean) => {
    if (!controlled) setOwn(o);
    onOpenChange?.(o);
  };
  const router = useRouter();
  const create = useCreateWorkspace(orgId);
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) create.reset();
      }}
    >
      {controlled ? null : (
        <DialogTrigger asChild>
          <Button>New workspace</Button>
        </DialogTrigger>
      )}
      <DialogContent title="New workspace" description="Each workspace comes with its own Daneel agent.">
        <CreateWorkspaceForm
          pending={create.isPending}
          error={create.error}
          onSubmit={(body) =>
            create.mutate(body, {
              onSuccess: (res) => {
                setOpen(false);
                router.push(`/o/${orgSlug}/w/${res.workspace.slug}`);
              },
            })
          }
        />
      </DialogContent>
    </Dialog>
  );
}
