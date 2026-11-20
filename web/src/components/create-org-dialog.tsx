"use client";

import { useRouter } from "next/navigation";

import { useCreateOrg } from "@/lib/api/queries";

import { CreateOrgForm } from "./create-org-form";
import { Dialog, DialogContent } from "./ui/dialog";

/** (org-5) A new organization from the switcher or the not-found card: Name and address, then its first workspace. */
export function CreateOrgDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const router = useRouter();
  const create = useCreateOrg();
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        onOpenChange(o);
        if (!o) create.reset();
      }}
    >
      <DialogContent title="New organization" description="You become its owner. It starts with one workspace and one agent.">
        <CreateOrgForm
          pending={create.isPending}
          error={create.error}
          onSubmit={(b) =>
            create.mutate(b, {
              onSuccess: (r) => {
                onOpenChange(false);
                router.push(`/o/${r.org.slug}/w/${r.workspace.slug}`);
              },
            })
          }
        />
      </DialogContent>
    </Dialog>
  );
}
