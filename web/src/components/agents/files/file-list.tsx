"use client";

import Link from "next/link";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { FileIcon, FileTextIcon, FolderIcon, ImageIcon, LinkIcon, MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { cn } from "@/lib/utils";
import { downloadUrl, effectiveKind, humanSize, relativeTime, unopenable, type FileEntry } from "@/lib/api/files";

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
}

function NameCell({ entry, onOpen }: { entry: FileEntry; onOpen: (e: FileEntry) => void }) {
  const blocked = unopenable(entry);
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
        {!entry.writable ? (
          <Badge className="mt-0.5">Read-only</Badge>
        ) : null}
      </div>
    </div>
  );
}

function RowMenu({ agent, entry, actions }: { agent: string; entry: FileEntry } & { actions: FileListActions }) {
  const openable = unopenable(entry) === null;
  const downloadable = openable && effectiveKind(entry) === "file";
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
            <a href={downloadUrl(agent, entry.path)} download={entry.name}>
              Download
            </a>
          </MenuItem>
        ) : null}
        {entry.writable ? <MenuItem onSelect={() => actions.onRename(entry)}>Rename</MenuItem> : null}
        {entry.writable ? (
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
export function FileList({ agent, entries, actions, now }: { agent: string; entries: FileEntry[]; actions: FileListActions; now?: number }) {
  return (
    <DataTable caption="Files in this folder" className="max-md:block max-md:min-w-0">
      <thead className="max-md:sr-only">
        <tr>
          <Th>Name</Th>
          <Th className="text-right">Size</Th>
          <Th>Modified</Th>
          <Th>
            <span className="sr-only">Actions</span>
          </Th>
        </tr>
      </thead>
      <tbody className="max-md:block">
        {entries.map((e) => {
          return (
            <tr key={e.path} className="max-md:flex max-md:flex-wrap max-md:items-center max-md:gap-x-3 max-md:border-t max-md:border-line max-md:px-3 max-md:py-2">
              <Td className="max-md:order-1 max-md:basis-3/4 max-md:flex-1 max-md:border-0 max-md:p-0">
                <NameCell entry={e} onOpen={actions.onOpen} />
              </Td>
              <Td className={cn("whitespace-nowrap text-right tabular-nums text-ink-2", "max-md:order-3 max-md:border-0 max-md:p-0 max-md:text-xs max-md:text-muted")}>
                {e.kind === "dir" || e.kind === "symlink" ? "—" : humanSize(e.size)}
              </Td>
              <Td className="whitespace-nowrap text-ink-2 max-md:order-4 max-md:border-0 max-md:p-0 max-md:text-xs max-md:text-muted">
                <span title={e.modified_at ? new Date(e.modified_at).toLocaleString() : undefined}>{relativeTime(e.modified_at, now)}</span>
              </Td>
              <Td className="w-px text-right max-md:order-2 max-md:border-0 max-md:p-0">
                <RowMenu agent={agent} entry={e} actions={actions} />
              </Td>
            </tr>
          );
        })}
      </tbody>
    </DataTable>
  );
}
