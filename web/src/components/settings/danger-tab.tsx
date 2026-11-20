"use client";

import { DangerZone } from "@/components/ui/section";
import { DeleteWithName } from "@/components/ui/type-to-confirm";
import { deleteWorkspace } from "@/lib/api/account";

/**
 * Delete the workspace (workspace-8): a dialog with the consequence and "Type the workspace name to confirm: <name>"
 * with the **visible** name. The API's own confirmation is the slug, which the page sends itself.
 */
export function DangerTab({
  wsId,
  slug,
  name,
  onDeleted,
}: {
  wsId: string;
  slug: string;
  /** The workspace's visible name. */
  name: string;
  onDeleted: () => void;
}) {
  return (
    <DangerZone
      id="delete-workspace"
      title="Delete this workspace"
      description="This permanently deletes the workspace and everything in it."
    >
      <DeleteWithName
        buttonLabel="Delete workspace"
        size="lg"
        title={`Delete ${name}?`}
        consequence="This deletes the workspace, its agents, files and conversations. It can't be undone."
        noun="workspace"
        name={name}
        run={() => deleteWorkspace(wsId, slug)}
        onDone={onDeleted}
      />
    </DangerZone>
  );
}
