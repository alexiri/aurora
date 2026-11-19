"use client";

import { DangerZone } from "@/components/ui/section";
import { TypeToConfirmDelete } from "@/components/ui/type-to-confirm-delete";
import { deleteWorkspace } from "@/lib/api/account";

export function DangerTab({
  wsId,
  slug,
  onDeleted,
}: {
  wsId: string;
  slug: string;
  onDeleted: () => void;
}) {
  return (
    <DangerZone
      id="delete-workspace"
      title="Delete this workspace"
      description={
        <>
          This permanently deletes the workspace and everything in it. Type <code className="font-mono">{slug}</code> to
          confirm.
        </>
      }
    >
      <TypeToConfirmDelete
        id="del-ws"
        fieldLabel="Workspace URL name"
        slug={slug}
        buttonLabel="Delete workspace"
        size="lg"
        remove={(confirm) => deleteWorkspace(wsId, confirm)}
        onDeleted={onDeleted}
      />
    </DangerZone>
  );
}
