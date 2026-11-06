"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { useCreateWorkspace } from "@/lib/api/queries";

import { CreateWorkspaceForm } from "./create-workspace-form";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogTrigger } from "./ui/dialog";

export function CreateWorkspaceDialog({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const [open, setOpen] = useState(false);
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
      <DialogTrigger asChild>
        <Button>New workspace</Button>
      </DialogTrigger>
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
