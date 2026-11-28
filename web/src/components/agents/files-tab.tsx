"use client";

// The agent's Files page (P5-FILES-2, D-316, C-904): browse, upload, download and manage `/workspace` over the API's
// SFTP-backed routes. It exists exactly where the Terminal page does (hosted, `terminal.enabled`, `can_use`) and is that
// privilege with another interface. Names and bytes are only ever rendered as text; downloads are plain links to the
// attachment route (SEC-126). docs/14 §Files is the source of truth for the layout.
import { useQueryClient } from "@tanstack/react-query";
import { formatCount } from "@/lib/format-date";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { EmptyState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Breadcrumb } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { DropZone } from "@/components/ui/drop-zone";
import { CheckboxField } from "@/components/ui/form";
import { FolderIcon, PlusIcon, UploadIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { Notice } from "@/components/ui/notice";
import { useUndoToast } from "@/components/ui/undo-toast";
import { UploadTray, type UploadTrayItem } from "@/components/ui/upload-tray";
import { useAltitude } from "@/lib/altitude";
import {
  LIST_MAX,
  WORKSPACE_ROOT,
  baseName,
  breadcrumbs,
  filesErrorMessage,
  filesKeys,
  isAgentOwn,
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
  UNAVAILABLE_ATTEMPTS,
  type FileEntry,
} from "@/lib/api/files";
import { ApiError } from "@/lib/api/client";

import { fromDataTransfer, fromFileList } from "./files/collect";
import { ClashDialog, DeleteDialog, GoToDialog, HiddenEditDialog, MoveDialog, NameDialog, type NameDialogKind } from "./files/dialogs";
import { FileList } from "./files/file-list";
import { useLeaveGate } from "./files/leave-gate";
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

/** A path with a hidden segment under `/workspace` (`.claude/settings.json`). */
const inHiddenPlace = (path: string) => path.split("/").filter(Boolean).slice(1).some((s) => s.startsWith("."));

export function FilesTab({ agentId, agentName, frozen, onOpenInstructions }: { agentId: string; agentName?: string; frozen: boolean; onOpenInstructions?: () => void }) {
  if (frozen) {
    // The freeze banner (with Unfreeze) is the agent header's; no listing and no request while frozen (D-316(d)).
    return (
      <Notice tone="neutral" role="status">
        This agent is paused. Files are unavailable while it is paused.
      </Notice>
    );
  }
  return <FilesBrowser agentId={agentId} agentName={agentName} onOpenInstructions={onOpenInstructions} />;
}

function FilesBrowser({ agentId, agentName, onOpenInstructions }: { agentId: string; agentName?: string; onOpenInstructions?: () => void }) {
  const qc = useQueryClient();
  const toast = useUndoToast();
  const altitude = useAltitude();
  const [path, setPath] = useFilesPath();
  const list = useFileList(agentId, path);
  // The tab renders only after the agent page's queries resolved, so it is never server-rendered: reading storage here is safe.
  const [hiddenOn, setShowHidden] = useState(readHidden);
  // (computer-9) Show hidden is an Expert control: without the switch nothing hidden is listed, whatever this browser stored.
  const showHidden = hiddenOn && altitude.allows("expert");
  const [goTo, setGoTo] = useState(false);
  const [nameDialog, setNameDialog] = useState<{ kind: NameDialogKind; entry?: FileEntry } | null>(null);
  const [deleting, setDeleting] = useState<FileEntry | null>(null);
  const [moving, setMoving] = useState<FileEntry | null>(null);
  const [viewing, setViewing] = useState<{ path: string; size?: number; readOnly?: boolean; isNew?: boolean } | null>(null);
  // The first hidden file opened for editing asks once; the answer holds for this visit.
  const [pendingHidden, setPendingHidden] = useState<FileEntry | null>(null);
  const hiddenConfirmed = useRef(false);
  const leave = useLeaveGate();
  const [actionError, setActionError] = useState<string | null>(null);
  const filesInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const name = agentName ?? "the agent";

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
  // (computer-10) A toast for each finished upload.
  const told = useRef(new Set<string>());
  useEffect(() => {
    for (const j of uploads.jobs) {
      if (j.state === "done" && !told.current.has(j.id)) {
        told.current.add(j.id);
        toast(`Uploaded ${j.label}`);
      }
    }
  }, [uploads.jobs, toast]);

  const goNow = (to: string) => {
    setViewing(null);
    setActionError(null);
    setPath(to);
  };
  const go = (to: string) => leave.ask(() => goNow(to));
  const edit = (entry: FileEntry, at: string) => setViewing({ path: at, size: entry.size, readOnly: !entry.writable || isManaged(at) || isAgentOwn(at) });
  const open = (entry: FileEntry) => {
    const target = entry.kind === "symlink" && entry.target ? entry.target : null;
    const kind = effectiveKind(entry);
    const at = target ? target.path : entry.path;
    if (kind === "dir") go(at);
    else if (kind === "file") {
      // (computer-9) A hidden file that can be edited asks once before it opens.
      if (inHiddenPlace(at) && entry.writable && !isManaged(at) && !hiddenConfirmed.current) setPendingHidden(entry);
      else leave.ask(() => edit(entry, at));
    }
  };

  const entries = useMemo<FileEntry[]>(() => list.data?.entries ?? [], [list.data]);
  const taken = useMemo(() => new Set(entries.map((e) => e.name)), [entries]);
  const visible = showHidden ? entries : entries.filter((e) => !isHidden(e));

  const startUploads = (items: ReturnType<typeof fromFileList>) => queue.enqueue(items);
  const dest = () => path;

  const listError = list.error;
  const fileFromUrl = listError instanceof ApiError && listError.code === "not_a_directory";
  const shownFile = viewing ?? (fileFromUrl ? { path } : null);
  const trail = breadcrumbs(shownFile ? shownFile.path : path).map((t) => (t.path === WORKSPACE_ROOT ? { ...t, label: `Your files with ${name}` } : t));
  const waking = list.isFetching && list.failureCount > 0 && isTransient(list.failureReason);
  // (computer-2) The listing could not load: Upload and New file stay in place, disabled, with the reason.
  const failed = !!listError && !fileFromUrl && !(listError instanceof ApiError && listError.code === "files_not_supported");
  const loaded = !!list.data;

  const copyPath = async (entry: FileEntry) => {
    try {
      await navigator.clipboard.writeText(entry.path);
    } catch {
      setActionError("Copying is not allowed in this browser. The path is " + entry.path);
    }
  };

  const moveTo = async (entry: FileEntry, dir: string) => {
    await renameOp(agentId, entry.path, joinPath(dir, entry.name));
    refresh();
    toast(`Moved ${entry.name} to ${dir === WORKSPACE_ROOT ? baseName(WORKSPACE_ROOT) : baseName(dir)}`);
  };

  return (
    <div className="min-w-0 max-w-full space-y-3">
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
        />
      ) : (
        <>
          <div role="group" aria-label="File actions" className="flex flex-wrap items-center gap-2">
            <Menu modal={false}>
              <MenuTrigger asChild>
                <Button type="button" variant="primary" size="sm" disabled={!loaded}>
                  <UploadIcon /> Upload
                </Button>
              </MenuTrigger>
              <MenuContent>
                <MenuItem onSelect={() => filesInput.current?.click()}>Files…</MenuItem>
                <MenuItem onSelect={() => folderInput.current?.click()}>Folder…</MenuItem>
              </MenuContent>
            </Menu>
            <Button type="button" variant="outline" size="sm" disabled={!loaded} onClick={() => setNameDialog({ kind: "folder" })}>
              <FolderIcon /> New folder
            </Button>
            <Button type="button" variant="outline" size="sm" disabled={!loaded} onClick={() => setNameDialog({ kind: "file" })}>
              <PlusIcon /> New file
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => void list.refetch()} disabled={list.isFetching}>
              Refresh
            </Button>
            <Altitude id="files.hidden" level="expert">
              <CheckboxField id="files-hidden" label="Show hidden" checked={hiddenOn} onChange={toggleHidden} />
            </Altitude>
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
          {failed ? (
            <p className="text-xs text-muted" data-testid="files-nothing-loaded">
              Nothing loaded. Actions come back when the computer answers.
            </p>
          ) : null}

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
            disabled={!loaded}
            onDataTransfer={(dt) => {
              // The entries are read synchronously inside the event; the tree is walked afterwards.
              const target = dest();
              void fromDataTransfer(dt, target).then(startUploads);
            }}
            fillPane
            className="min-h-32 md:min-h-72">
            {list.isPending && !listError ? (
              waking ? (
                <Notice tone="pending" role="status">
                  {list.failureReason instanceof ApiError && list.failureReason.code === "files_unavailable"
                    ? `Reconnecting… (attempt ${Math.min(list.failureCount + 1, UNAVAILABLE_ATTEMPTS)} of ${UNAVAILABLE_ATTEMPTS})`
                    : "Waking the computer…"}
                </Notice>
              ) : (
                <Loading label="Loading files…" />
              )
            ) : null}
            {listError ? (
              <FilesError
                error={listError}
                tries={list.failureCount}
                path={path}
                onRetry={() => void list.refetch()}
                onRoot={() => go(WORKSPACE_ROOT)}
              />
            ) : null}
            {list.data ? (
              <div className="space-y-3">
                {path === WORKSPACE_ROOT && visible.some((e) => isAgentOwn(e.path)) ? (
                  <Notice tone="neutral" role="note" data-testid="own-note">
                    The grey files are {name}&apos;s instructions and memory. Aurora keeps them for you; edit them on the{" "}
                    {onOpenInstructions ? (
                      <button type="button" className="text-accent-ink underline underline-offset-2" onClick={onOpenInstructions}>
                        Instructions page
                      </button>
                    ) : (
                      "Instructions page"
                    )}
                    .
                  </Notice>
                ) : null}
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
                    ownGroup={path === WORKSPACE_ROOT ? { name, collapsedOnPhone: true } : undefined}
                    actions={{
                      onOpen: open,
                      onRename: (entry) => setNameDialog({ kind: "rename", entry }),
                      onDelete: setDeleting,
                      onCopyPath: (entry) => void copyPath(entry),
                      onMove: setMoving,
                      onDropMove: (from, folder) => {
                        const entry = entries.find((e) => e.path === from);
                        if (!entry || entry.path === folder.path) return;
                        void moveTo(entry, folder.path).catch((e: unknown) => setActionError(filesErrorMessage(e)));
                      },
                      onDownloaded: (entry) => toast(`Downloaded ${entry.name}`),
                    }}
                  />
                )}
                {showHidden ? (
                  <Notice tone="pending" role="note" data-testid="hidden-warning">
                    <strong>Hidden files</strong> hold settings Aurora and the agent rely on. Edit them only if you know why.
                  </Notice>
                ) : null}
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
        onSubmit={async (value) => {
          const kind = nameDialog?.kind;
          if (kind === "folder") await mkdirOp(agentId, joinPath(path, value));
          else if (kind === "file") {
            // An empty editor: nothing is created until its first Save (`If-None-Match: *`).
            setViewing({ path: joinPath(path, value), isNew: true });
            return;
          } else if (kind === "rename" && nameDialog?.entry) {
            await renameOp(agentId, nameDialog.entry.path, joinPath(parentPath(nameDialog.entry.path), value));
            toast(`Renamed to ${value}`);
          }
          refresh();
        }}
      />
      <MoveDialog agent={agentId} entry={moving} onOpenChange={(o) => !o && setMoving(null)} onMove={moveTo} />
      <HiddenEditDialog
        name={pendingHidden?.name ?? null}
        onCancel={() => setPendingHidden(null)}
        onConfirm={() => {
          const entry = pendingHidden;
          hiddenConfirmed.current = true;
          setPendingHidden(null);
          if (entry) leave.ask(() => edit(entry, entry.kind === "symlink" && entry.target ? entry.target.path : entry.path));
        }}
      />
      {leave.dialog}
      <DeleteDialog
        entry={deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        onConfirm={async (entry) => {
          await deleteOp(agentId, entry.path, entry.kind === "dir");
          refresh();
          // A delete cannot be taken back (the API has no restore), so the toast has no Undo and the confirm dialog stays.
          toast(`Deleted ${entry.name}`);
        }}
      />
      <ClashDialog job={uploads.clash} remaining={uploads.remaining} onChoose={(choice, all) => queue.resolveClash(choice, all)} />
    </div>
  );
}

/** The listing's failure states (docs/14 §Files item 7): each error code in words, with the action that helps. */
function FilesError({ error, tries, path, onRetry, onRoot }: { error: unknown; tries: number; path: string; onRetry: () => void; onRoot: () => void }) {
  if (error instanceof ApiError && error.code === "not_a_directory") return <Loading label="Opening…" />;
  if (error instanceof ApiError && error.code === "files_not_supported") {
    return (
      <Notice tone="neutral" role="status">
        {filesErrorMessage(error)}
      </Notice>
    );
  }
  const transient = isTransient(error);
  const gone = error instanceof ApiError && (error.code === "path_not_found" || error.code === "path_outside_workspace" || error.code === "invalid_path");
  return (
    <Notice tone="danger" role="alert" className="space-y-2">
      {transient ? (
        <>
          {/* (computer-2) The retries ran out: say so, once, with the way out. */}
          <p className="font-semibold">Couldn&apos;t connect to the computer</p>
          <p className="break-words">Tried {tries} times. The computer didn&apos;t answer.</p>
        </>
      ) : (
        <p className="break-words">{filesErrorMessage(error)}</p>
      )}
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
