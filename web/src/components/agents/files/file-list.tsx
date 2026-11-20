"use client";

import Link from "next/link";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { FileIcon, FileTextIcon, FolderIcon, ImageIcon, LinkIcon, LockIcon, MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { When } from "@/components/ui/when";
import { agentOwnNote, downloadUrl, effectiveKind, humanSize, isAgentOwn, isManaged, unopenable, type FileEntry } from "@/lib/api/files";
import { formatExact, formatWhen } from "@/lib/format-date";
import { cn } from "@/lib/utils";

import { filesHref } from "./use-files-path";

const IMAGE = /\.(png|jpe?g|gif|webp|svg|bmp|ico)$/i;
const TEXT = /\.(txt|md|json|ya?ml|toml|rs|py|js|jsx|ts|tsx|css|html?|sh|log|csv|ini|cfg|conf|lock)$/i;

function EntryIcon({ entry }: { entry: FileEntry }) {
  const cls = "size-5 shrink-0 text-ink-2";
  if (entry.kind === "symlink") return <LinkIcon className={cls} />;
  if (entry.kind === "dir") return <FolderIcon className={cls} />;
  if (IMAGE.test(entry.name)) return <ImageIcon className={cls} />;
  if (TEXT.test(entry.name)) return <FileTextIcon className={cls} />;
  return <FileIcon className={cls} />;
}

export interface FileListActions {
  onOpen: (entry: FileEntry) => void;
  onRename: (entry: FileEntry) => void;
  onDelete: (entry: FileEntry) => void;
  onCopyPath: (entry: FileEntry) => void;
  /** (P5-REFINE-15) **Move to…**: opens the folder dialog. */
  onMove: (entry: FileEntry) => void;
  /** (P5-REFINE-15) A file row dropped onto a folder row: the same move. */
  onDropMove: (from: string, folder: FileEntry) => void;
  /** (computer-10) A download was started from the row menu: "Downloaded fruit.csv". */
  onDownloaded?: (entry: FileEntry) => void;
}

/** The drag type of a file row dragged onto a folder row (a drag from outside carries `Files` and uploads instead). */
const MOVE_TYPE = "application/x-aurora-file";

function NameCell({ entry, onOpen }: { entry: FileEntry; onOpen: (e: FileEntry) => void }) {
  const blocked = unopenable(entry);
  const own = agentOwnNote(entry.path);
  return (
    <div className="flex min-w-0 items-start gap-2.5">
      <span className="mt-0.5">
        <EntryIcon entry={entry} />
      </span>
      <div className="min-w-0">
        {blocked ? (
          <span className="break-all font-medium text-ink">{entry.name}</span>
        ) : effectiveKind(entry) === "dir" ? (
          <Link
            href={filesHref(entry.kind === "symlink" ? entry.target!.path : entry.path)}
            onClick={(e) => {
              if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
              e.preventDefault();
              onOpen(entry);
            }}
            className="break-all font-medium text-ink hover:underline">
            {entry.name}
          </Link>
        ) : (
          <button type="button" onClick={() => onOpen(entry)} className="break-all text-left font-medium text-ink hover:underline">
            {entry.name}
          </button>
        )}
        {entry.kind === "symlink" ? (
          <p className="break-all font-mono text-xs text-muted">
            → {entry.target?.path ?? "?"}
            {blocked ? ` · ${blocked}` : ""}
          </p>
        ) : blocked ? (
          <p className="text-xs text-muted">{blocked}</p>
        ) : null}
        {own ? <p className="text-xs text-muted">{own}</p> : null}
        {/* (computer-9) Aurora's own entries say why they are read-only: Managed, never a bare Read-only. */}
        {own || isManaged(entry.path) ? (
          <Badge className="mt-0.5">
            <LockIcon width={12} height={12} /> Managed
          </Badge>
        ) : !entry.writable ? (
          <Badge className="mt-0.5">Read-only</Badge>
        ) : null}
      </div>
    </div>
  );
}

function RowMenu({ agent, entry, actions }: { agent: string; entry: FileEntry } & { actions: FileListActions }) {
  const openable = unopenable(entry) === null;
  const downloadable = openable && effectiveKind(entry) === "file";
  // Writable entries only: Aurora's own entries and `.aurora` are not renamed, moved or deleted from here.
  const writable = entry.writable && !isManaged(entry.path) && !isAgentOwn(entry.path);
  return (
    <Menu modal={false}>
      <MenuTrigger asChild>
        <Button variant="ghost" size="sm" aria-label={`Actions for ${entry.name}`} className="size-11 min-h-11 px-0 md:size-9 md:min-h-9">
          <MoreIcon width={20} height={20} />
        </Button>
      </MenuTrigger>
      <MenuContent align="end">
        {openable ? <MenuItem onSelect={() => actions.onOpen(entry)}>Open</MenuItem> : null}
        {downloadable ? (
          // A plain same-origin link to the attachment route: never window.open, never a blob: navigation (SEC-126).
          <MenuItem asChild>
            <a href={downloadUrl(agent, entry.path)} download={entry.name} onClick={() => actions.onDownloaded?.(entry)}>
              Download
            </a>
          </MenuItem>
        ) : null}
        {writable ? <MenuItem onSelect={() => actions.onRename(entry)}>Rename</MenuItem> : null}
        {writable ? <MenuItem onSelect={() => actions.onMove(entry)}>Move to…</MenuItem> : null}
        {writable ? (
          <MenuItem className="text-danger-ink data-[highlighted]:bg-danger-soft" onSelect={() => actions.onDelete(entry)}>
            Delete
          </MenuItem>
        ) : null}
        <MenuItem onSelect={() => actions.onCopyPath(entry)}>Copy path</MenuItem>
      </MenuContent>
    </Menu>
  );
}

/**
 * The folder's entries (docs/14 §Files item 3): a `DataTable` that becomes a stack of cards at 390 px (the same markup:
 * rows turn into flex cards below `md`, name and menu on the first line, size and time on the second).
 */
export function FileList({
  agent,
  entries,
  actions,
  now,
  ownGroup,
}: {
  agent: string;
  entries: FileEntry[];
  actions: FileListActions;
  now?: number;
  /** (computer-9) The workspace root: Aurora's entries go in their own group, named for the agent ("Dani's own"). */
  ownGroup?: { name: string; collapsedOnPhone?: boolean };
}) {
  // A phone starts with the agent's own group closed; read once when the list mounts (the page renders client-side only).
  const [ownOpen, setOwnOpen] = useState(() => !(ownGroup?.collapsedOnPhone && typeof window !== "undefined" && window.matchMedia?.("(max-width: 639px)").matches));
  const [dropOn, setDropOn] = useState<string | null>(null);
  const own = ownGroup ? entries.filter((e) => isAgentOwn(e.path)) : [];
  const mine = ownGroup ? entries.filter((e) => !isAgentOwn(e.path)) : entries;
  const row = (e: FileEntry) => {
    const draggable = e.writable && !isManaged(e.path) && !isAgentOwn(e.path);
    const folder = e.kind === "dir" && !isManaged(e.path);
    return (
      <tr
        key={e.path}
        data-testid={`file-row-${e.name}`}
        data-drop={dropOn === e.path || undefined}
        className={cn(dropOn === e.path && "bg-accent-soft", own.includes(e) && "text-ink-2")}
        draggable={draggable || undefined}
        onDragStart={
          draggable
            ? (ev) => {
                ev.dataTransfer.setData(MOVE_TYPE, e.path);
                ev.dataTransfer.effectAllowed = "move";
              }
            : undefined
        }
        onDragOver={
          folder
            ? (ev) => {
                if (!Array.from(ev.dataTransfer.types).includes(MOVE_TYPE)) return;
                ev.preventDefault();
                ev.dataTransfer.dropEffect = "move";
                setDropOn(e.path);
              }
            : undefined
        }
        onDragLeave={folder ? () => setDropOn((p) => (p === e.path ? null : p)) : undefined}
        onDrop={
          folder
            ? (ev) => {
                const from = ev.dataTransfer.getData(MOVE_TYPE);
                setDropOn(null);
                if (!from) return;
                ev.preventDefault();
                actions.onDropMove(from, e);
              }
            : undefined
        }>
        <Td cell="title">
          <NameCell entry={e} onOpen={actions.onOpen} />
        </Td>
        <Td className="whitespace-nowrap text-right tabular-nums text-ink-2 max-stack:text-xs max-stack:text-muted">
          {e.kind === "dir" || e.kind === "symlink" ? "—" : humanSize(e.size)}
        </Td>
        <Td className="whitespace-nowrap text-ink-2 max-stack:text-xs max-stack:text-muted">
          {now === undefined ? <When value={e.modified_at} /> : <span title={e.modified_at ? formatExact(e.modified_at) : undefined}>{formatWhen(e.modified_at, { now })}</span>}
        </Td>
        <Td cell="actions" className="w-px text-right">
          <RowMenu agent={agent} entry={e} actions={actions} />
        </Td>
      </tr>
    );
  };
  const header = (title: string, toggle?: { open: boolean; onToggle: () => void }) => (
    <tr data-group={title}>
      <Td colSpan={4} className="bg-canvas py-1.5 text-xs font-semibold uppercase tracking-label text-muted">
        {toggle ? (
          <button type="button" aria-expanded={toggle.open} className="flex min-h-9 w-full items-center justify-between gap-2 text-left uppercase" onClick={toggle.onToggle}>
            <span>{title}</span>
            <span aria-hidden="true">{toggle.open ? "−" : "+"}</span>
          </button>
        ) : (
          title
        )}
      </Td>
    </tr>
  );
  if (ownGroup && own.length > 0) {
    return (
      <DataTable caption="Files in this folder" stackBelow={900}>
        <thead>
          <tr>
            <Th>Name</Th>
            <Th className="text-right">Size</Th>
            <Th>Modified</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {mine.length > 0 ? header("Your files") : null}
          {mine.map(row)}
          {header(`${ownGroup.name}'s own`, { open: ownOpen, onToggle: () => setOwnOpen((o) => !o) })}
          {ownOpen ? own.map(row) : null}
        </tbody>
      </DataTable>
    );
  }
  return (
    <DataTable caption="Files in this folder" stackBelow={900}>
      <thead>
        <tr>
          <Th>Name</Th>
          <Th className="text-right">Size</Th>
          <Th>Modified</Th>
          <Th>
            <span className="sr-only">Actions</span>
          </Th>
        </tr>
      </thead>
      <tbody>
        {entries.map(row)}
      </tbody>
    </DataTable>
  );
}
