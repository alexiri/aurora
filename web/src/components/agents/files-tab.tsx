"use client";

// The agent's Files page (P5-FILES-2, D-316, C-904): browse, upload, download and manage `/workspace` over the API's
// SFTP-backed routes. It exists exactly where the Terminal page does (hosted, `terminal.enabled`, `can_use`) and is that
// privilege with another interface. Names and bytes are only ever rendered as text; downloads are plain links to the
// attachment route (SEC-126). docs/14 §Files is the source of truth for the layout.
import { useQueryClient } from "@tanstack/react-query";
import { formatCount } from "@/lib/format-date";
import { useMemo, useRef, useState, useSyncExternalStore } from "react";

import { EmptyState, Loading } from "@/components/states";
import { Breadcrumb } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DropZone } from "@/components/ui/drop-zone";
import { CheckboxField } from "@/components/ui/form";
import { FolderIcon, PlusIcon, UploadIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { Notice } from "@/components/ui/notice";
import { UploadTray, type UploadTrayItem } from "@/components/ui/upload-tray";
import {
  LIST_MAX,
  WORKSPACE_ROOT,
  breadcrumbs,
  filesErrorMessage,
  filesKeys,
  isHidden,
  isManaged,
  isTransient,
  joinPath,
  mkdirOp,
  parentPath,
  putFile,
  renameOp,
  deleteOp,
  effectiveKind,
  useFileList,
  type FileEntry,
} from "@/lib/api/files";
import { ApiError } from "@/lib/api/client";

import { fromDataTransfer, fromFileList } from "./files/collect";
import { ClashDialog, DeleteDialog, GoToDialog, NameDialog, type NameDialogKind } from "./files/dialogs";
import { FileList } from "./files/file-list";
import { FileViewer } from "./files/viewer/file-viewer";
import { UploadQueue, type UploadJob } from "./files/upload-queue";
import { filesHref, useFilesPath } from "./files/use-files-path";

const HIDDEN_KEY = "aurora.files.hidden";

function readHidden(): boolean {
  try {
    return window.localStorage.getItem(HIDDEN_KEY) === "1";
  } catch {
    return false;
  }
}

const trayState = (j: UploadJob): UploadTrayItem["state"] => j.state;

export function FilesTab({ agentId, frozen }: { agentId: string; frozen: boolean }) {
  if (frozen) {
    // The freeze banner (with Unfreeze) is the agent header's; no listing and no request while frozen (D-316(d)).
    return (
      <Notice tone="neutral" role="status">
        This agent is paused. Files are unavailable while it is paused.
      </Notice>
    );
  }
  return <FilesBrowser agentId={agentId} />;
}

function FilesBrowser({ agentId }: { agentId: string }) {
  const qc = useQueryClient();
  const [path, setPath] = useFilesPath();
  const list = useFileList(agentId, path);
  // The tab renders only after the agent page's queries resolved, so it is never server-rendered: reading storage here is safe.
  const [showHidden, setShowHidden] = useState(readHidden);
  const [goTo, setGoTo] = useState(false);
  const [nameDialog, setNameDialog] = useState<{ kind: NameDialogKind; entry?: FileEntry } | null>(null);
  const [deleting, setDeleting] = useState<FileEntry | null>(null);
  const [viewing, setViewing] = useState<{ path: string; size?: number; readOnly?: boolean; isNew?: boolean } | null>(null);
  // The viewer's unsaved changes: leaving the file by any in-page navigation asks first (P5-FILES-3).
  const [dirty, setDirty] = useState(false);
  const [pendingNav, setPendingNav] = useState<(() => void) | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const filesInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);

  const toggleHidden = (on: boolean) => {
    setShowHidden(on);
    try {
      window.localStorage.setItem(HIDDEN_KEY, on ? "1" : "0");
    } catch {
      // Private mode or blocked storage: the switch still works for this visit.
    }
  };

  const refresh = () => void qc.invalidateQueries({ queryKey: filesKeys.all(agentId) });
  const queue = useMemo(
    () =>
      new UploadQueue(
        { put: (to, body, o) => putFile(agentId, to, body, o), mkdirParents: (dir) => mkdirOp(agentId, dir, true) },
        { onDrained: () => void qc.invalidateQueries({ queryKey: filesKeys.all(agentId) }) },
      ),
    [agentId, qc],
  );
  const uploads = useSyncExternalStore(queue.subscribe, queue.getSnapshot, queue.getSnapshot);

  const goNow = (to: string) => {
    setViewing(null);
    setActionError(null);
    setPath(to);
  };
  const go = (to: string) => (dirty ? setPendingNav(() => () => goNow(to)) : goNow(to));
  const open = (entry: FileEntry) => {
    const target = entry.kind === "symlink" && entry.target ? entry.target : null;
    const kind = effectiveKind(entry);
    const at = target ? target.path : entry.path;
    if (kind === "dir") go(at);
    else if (kind === "file") setViewing({ path: at, size: entry.size, readOnly: !entry.writable || isManaged(at) });
  };

  const entries = useMemo<FileEntry[]>(() => list.data?.entries ?? [], [list.data]);
  const taken = useMemo(() => new Set(entries.map((e) => e.name)), [entries]);
  const visible = showHidden ? entries : entries.filter((e) => !isHidden(e));

  const startUploads = (items: ReturnType<typeof fromFileList>) => queue.enqueue(items);
  const dest = () => path;

  const listError = list.error;
  const fileFromUrl = listError instanceof ApiError && listError.code === "not_a_directory";
  const shownFile = viewing ?? (fileFromUrl ? { path } : null);
  const trail = breadcrumbs(shownFile ? shownFile.path : path);
  const waking = list.isFetching && list.failureCount > 0 && isTransient(list.failureReason);

  const copyPath = async (entry: FileEntry) => {
    try {
      await navigator.clipboard.writeText(entry.path);
    } catch {
      setActionError("Copying is not allowed in this browser. The path is " + entry.path);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <Breadcrumb
          label="Folder path"
          className="min-w-0 flex-1"
          // The last segment is the current place (the folder shown, or the file being viewed): not a link.
          items={trail.map((t, i) => (i === trail.length - 1 ? { label: t.label } : { label: t.label, href: filesHref(t.path), onSelect: () => go(t.path) }))}
        />
        <Button type="button" variant="outline" size="sm" onClick={() => setGoTo(true)}>
          Go to path
        </Button>
      </div>

      {shownFile ? (
        <FileViewer
          key={`${shownFile.path}:${shownFile.isNew ? "new" : "open"}`}
          agent={agentId}
          path={shownFile.path}
          size={shownFile.size}
          readOnly={"readOnly" in shownFile ? shownFile.readOnly : isManaged(shownFile.path)}
          isNew={"isNew" in shownFile ? shownFile.isNew : false}
          onBack={() => go(parentPath(shownFile.path))}
          onDirtyChange={setDirty}
        />
      ) : (
        <>
          <div role="group" aria-label="File actions" className="flex flex-wrap items-center gap-2">
            <Menu modal={false}>
              <MenuTrigger asChild>
                <Button type="button" variant="primary" size="sm">
                  <UploadIcon /> Upload
                </Button>
              </MenuTrigger>
              <MenuContent>
                <MenuItem onSelect={() => filesInput.current?.click()}>Files…</MenuItem>
                <MenuItem onSelect={() => folderInput.current?.click()}>Folder…</MenuItem>
              </MenuContent>
            </Menu>
            <Button type="button" variant="outline" size="sm" onClick={() => setNameDialog({ kind: "folder" })}>
              <FolderIcon /> New folder
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => setNameDialog({ kind: "file" })}>
              <PlusIcon /> New file
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => void list.refetch()} disabled={list.isFetching}>
              Refresh
            </Button>
            <CheckboxField id="files-hidden" label="Show hidden" checked={showHidden} onChange={toggleHidden} />
            <input
              ref={filesInput}
              type="file"
              multiple
              hidden
              data-testid="files-input"
              aria-label="Choose files to upload"
              onChange={(e) => {
                startUploads(fromFileList(Array.from(e.target.files ?? []), dest()));
                e.target.value = "";
              }}
            />
            <input
              ref={folderInput}
              type="file"
              hidden
              data-testid="folder-input"
              aria-label="Choose a folder to upload"
              {...{ webkitdirectory: "" }}
              onChange={(e) => {
                startUploads(fromFileList(Array.from(e.target.files ?? []), dest()));
                e.target.value = "";
              }}
            />
          </div>

          <UploadTray
            items={uploads.jobs.map((j) => ({
              id: j.id,
              label: j.label,
              state: trayState(j),
              progress: j.total > 0 ? j.sent / j.total : 0,
              error: j.error,
              note: j.note,
            }))}
            onCancel={(id) => queue.cancel(id)}
            onDismiss={(id) => queue.dismiss(id)}
            onClear={() => queue.clearFinished()}
          />

          {actionError ? (
            <Notice tone="danger" role="alert" className="flex items-start justify-between gap-2">
              <span className="min-w-0 break-words">{actionError}</span>
              <Button type="button" variant="ghost" size="sm" onClick={() => setActionError(null)}>
                Dismiss
              </Button>
            </Notice>
          ) : null}

          <DropZone
            label={`Drop to upload to ${path}`}
            icon={<UploadIcon width={24} height={24} />}
            onDataTransfer={(dt) => {
              // The entries are read synchronously inside the event; the tree is walked afterwards.
              const target = dest();
              void fromDataTransfer(dt, target).then(startUploads);
            }}
            className="min-h-32">
            {list.isPending && !listError ? (
              waking ? (
                <Notice tone="pending" role="status">
                  {list.failureReason instanceof ApiError && list.failureReason.code === "files_unavailable"
                    ? "The computer restarted; retrying…"
                    : "Waking the computer…"}
                </Notice>
              ) : (
                <Loading label="Loading files…" />
              )
            ) : null}
            {listError ? (
              <FilesError
                error={listError}
                path={path}
                onRetry={() => void list.refetch()}
                onRoot={() => go(WORKSPACE_ROOT)}
              />
            ) : null}
            {list.data ? (
              <div className="space-y-3">
                {visible.length === 0 ? (
                  <EmptyState
                    title="This folder is empty"
                    action={
                      <>
                        <Button type="button" variant="primary" size="sm" onClick={() => filesInput.current?.click()}>
                          <UploadIcon /> Upload
                        </Button>
                        <Button type="button" variant="outline" size="sm" onClick={() => setNameDialog({ kind: "file" })}>
                          New file
                        </Button>
                      </>
                    }>
                    {entries.length > 0 ? "Everything here is hidden. Turn on Show hidden to see it." : "Drop files here or use Upload."}
                  </EmptyState>
                ) : (
                  <FileList
                    agent={agentId}
                    entries={visible}
                    actions={{
                      onOpen: open,
                      onRename: (entry) => setNameDialog({ kind: "rename", entry }),
                      onDelete: setDeleting,
                      onCopyPath: (entry) => void copyPath(entry),
                    }}
                  />
                )}
                {list.data.truncated ? (
                  <Notice tone="neutral" role="status">
                    Showing the first {formatCount(LIST_MAX)} items. Use the Terminal to see the rest.
                  </Notice>
                ) : null}
              </div>
            ) : null}
          </DropZone>
        </>
      )}

      <GoToDialog open={goTo} onOpenChange={setGoTo} current={path} onGo={go} />
      <NameDialog
        kind={nameDialog?.kind ?? "folder"}
        open={nameDialog !== null}
        onOpenChange={(o) => !o && setNameDialog(null)}
        initial={nameDialog?.entry?.name ?? ""}
        taken={taken}
        onSubmit={async (name) => {
          const kind = nameDialog?.kind;
          if (kind === "folder") await mkdirOp(agentId, joinPath(path, name));
          else if (kind === "file") {
            // An empty editor: nothing is created until its first Save (`If-None-Match: *`).
            setViewing({ path: joinPath(path, name), isNew: true });
            return;
          } else if (kind === "rename" && nameDialog?.entry) await renameOp(agentId, nameDialog.entry.path, joinPath(parentPath(nameDialog.entry.path), name));
          refresh();
        }}
      />
      <ConfirmDialog
        open={pendingNav !== null}
        onOpenChange={(o) => !o && setPendingNav(null)}
        title="Discard your changes?"
        description="You have unsaved changes in this file. Leaving now loses them."
        confirmLabel="Discard changes"
        danger
        onConfirm={() => {
          const next = pendingNav;
          setPendingNav(null);
          next?.();
        }}
      />
      <DeleteDialog
        entry={deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        onConfirm={async (entry) => {
          await deleteOp(agentId, entry.path, entry.kind === "dir");
          refresh();
        }}
      />
      <ClashDialog job={uploads.clash} remaining={uploads.remaining} onChoose={(choice, all) => queue.resolveClash(choice, all)} />
    </div>
  );
}

/** The listing's failure states (docs/14 §Files item 7): each error code in words, with the action that helps. */
function FilesError({ error, path, onRetry, onRoot }: { error: unknown; path: string; onRetry: () => void; onRoot: () => void }) {
  if (error instanceof ApiError && error.code === "not_a_directory") return <Loading label="Opening…" />;
  if (error instanceof ApiError && error.code === "files_not_supported") {
    return (
      <Notice tone="neutral" role="status">
        {filesErrorMessage(error)}
      </Notice>
    );
  }
  const transient = isTransient(error);
  const message = transient ? "The computer did not wake up in time. Try again in a moment." : filesErrorMessage(error);
  const gone = error instanceof ApiError && (error.code === "path_not_found" || error.code === "path_outside_workspace" || error.code === "invalid_path");
  return (
    <Notice tone="danger" role="alert" className="space-y-2">
      <p className="break-words">{message}</p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" onClick={onRetry}>
          Try again
        </Button>
        {gone && path !== WORKSPACE_ROOT ? (
          <Button type="button" variant="outline" size="sm" onClick={onRoot}>
            Go to {WORKSPACE_ROOT}
          </Button>
        ) : null}
      </div>
    </Notice>
  );
}
