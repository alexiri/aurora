// Turning what a person picked or dropped into upload queue items (P5-FILES-2): flat files, a folder chosen with
// `<input webkitdirectory>` (every file carries `webkitRelativePath`), or a dropped tree read through
// `DataTransferItem.webkitGetAsEntry()`. Folders are recreated before their files by the queue (`ensureDir`).
import { joinPath } from "@/lib/api/files";

import type { QueueItem } from "./upload-queue";

/** Files from a picker: with `webkitRelativePath` set they come from a folder and need their directories made. */
export function fromFileList(files: Iterable<File>, dest: string): QueueItem[] {
  const items: QueueItem[] = [];
  for (const file of files) {
    const rel = file.webkitRelativePath || file.name;
    const at = rel.lastIndexOf("/");
    const sub = at > 0 ? rel.slice(0, at) : "";
    items.push({ file, dir: sub ? joinPath(dest, sub) : dest, label: rel, ensureDir: sub !== "" });
  }
  return items;
}

function readAll(reader: FileSystemDirectoryReader): Promise<FileSystemEntry[]> {
  // `readEntries` returns at most a batch (100 in Chromium) per call; an empty batch ends the directory.
  return new Promise((resolve, reject) => {
    const out: FileSystemEntry[] = [];
    const next = () =>
      reader.readEntries((batch) => {
        if (batch.length === 0) resolve(out);
        else {
          out.push(...batch);
          next();
        }
      }, reject);
    next();
  });
}

const fileOf = (entry: FileSystemFileEntry) => new Promise<File>((resolve, reject) => entry.file(resolve, reject));

async function walk(entry: FileSystemEntry, rel: string, dest: string, out: QueueItem[]): Promise<void> {
  if (entry.isFile) {
    const file = await fileOf(entry as FileSystemFileEntry);
    const sub = rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "";
    out.push({ file, dir: sub ? joinPath(dest, sub) : dest, label: rel, ensureDir: sub !== "" });
    return;
  }
  if (!entry.isDirectory) return;
  const children = await readAll((entry as FileSystemDirectoryEntry).createReader());
  if (children.length === 0) out.push({ file: null, dir: joinPath(dest, rel), label: `${rel}/`, ensureDir: true });
  for (const child of children) await walk(child, `${rel}/${child.name}`, dest, out);
}

/**
 * The items of a drop. The entries must be taken synchronously (the browser empties the `DataTransfer` after the event),
 * so call this inside the drop handler; the tree is then read asynchronously. A browser without `webkitGetAsEntry`
 * falls back to the flat `files` list.
 */
export async function fromDataTransfer(data: DataTransfer, dest: string): Promise<QueueItem[]> {
  const entries: FileSystemEntry[] = [];
  let supported = true;
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== "file") continue;
    const entry = typeof item.webkitGetAsEntry === "function" ? item.webkitGetAsEntry() : null;
    if (entry) entries.push(entry);
    else supported = false;
  }
  if (!supported || entries.length === 0) return fromFileList(Array.from(data.files), dest);
  const out: QueueItem[] = [];
  for (const entry of entries) await walk(entry, entry.name, dest, out);
  return out;
}
