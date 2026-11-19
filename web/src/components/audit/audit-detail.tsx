"use client";

import { useState } from "react";

import { CopyValue } from "@/components/ui/copy-value";
import type { AuditEntry } from "./audit-panel";

// D-373, C-1027: what a row recorded. Everything is rendered as React text: no HTML, no Markdown, no links from metadata.

type Json = unknown;
const isObject = (v: Json): v is Record<string, Json> => typeof v === "object" && v !== null && !Array.isArray(v);

const PAIRS: [string, string][] = [
  ["from", "to"],
  ["before", "after"],
  ["old", "new"],
];

/** `{from,to}`, `{before,after}` or `{old,new}` with exactly those two keys. */
function pairOf(v: Json): [Json, Json] | null {
  if (!isObject(v)) return null;
  const keys = Object.keys(v);
  if (keys.length !== 2) return null;
  for (const [a, b] of PAIRS) if (keys.includes(a) && keys.includes(b)) return [v[a], v[b]];
  return null;
}

const label = (key: string) => key.replace(/_/g, " ");

/** A scalar as it reads; anything else as indented JSON. */
const text = (v: Json): string => (typeof v === "string" ? v : v === undefined ? "" : JSON.stringify(v, null, 2));
const isScalar = (v: Json) => v === null || typeof v !== "object";

const TEXT = "whitespace-pre-wrap break-words";

function Value({ v }: { v: Json }) {
  if (typeof v === "string" && v.endsWith("…")) {
    return (
      <span className={TEXT}>
        {v} <span className="block text-xs text-muted">Shortened when recorded</span>
      </span>
    );
  }
  if (isScalar(v)) return <span className={TEXT}>{v === null ? "none" : String(v)}</span>;
  return <pre className={`${TEXT} font-mono text-xs`}>{text(v)}</pre>;
}

/** One line `label: before → after`, wrapping. */
function BeforeAfter({ name, before, after }: { name: string; before: Json; after: Json }) {
  return (
    <p className={`${TEXT} text-sm`} data-testid="before-after">
      <span className="text-muted">{name}:</span> <span className="font-mono text-xs">{text(before) || "none"}</span>
      <span aria-hidden="true"> → </span>
      <span className="sr-only"> to </span>
      <span className="font-mono text-xs">{text(after) || "none"}</span>
    </p>
  );
}

function ChangeCell({
  side,
  before,
  after,
  whole,
  setWhole,
}: {
  side: "before" | "after";
  before: Json;
  after: Json;
  whole: boolean;
  setWhole: (v: boolean) => void;
}) {
  const mine = side === "before" ? before : after;
  const missing = mine === undefined || mine === null;
  if (missing) {
    return <span className="text-muted">{side === "before" ? "Added" : "Removed"}</span>;
  }
  if (isObject(before) && isObject(after) && !whole) {
    const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].filter((k) => JSON.stringify(before[k]) !== JSON.stringify(after[k]));
    if (side === "before") {
      return (
        <div className="space-y-1">
          {keys.map((k) => (
            <p key={k} className={`${TEXT} text-sm`}>
              <span className="text-muted">{k}:</span> <span className="font-mono text-xs">{text(before[k]) || "none"}</span>
            </p>
          ))}
        </div>
      );
    }
    return (
      <div className="space-y-1">
        {keys.map((k) => (
          <p key={k} className={`${TEXT} text-sm`}>
            <span className="text-muted">{k}:</span> <span className="font-mono text-xs">{text(after[k]) || "none"}</span>
          </p>
        ))}
        <button type="button" className="text-xs text-accent underline" aria-pressed={whole} onClick={() => setWhole(true)}>
          Show whole entry
        </button>
      </div>
    );
  }
  return (
    <div>
      <Value v={mine} />
      {side === "after" && isObject(before) && isObject(after) ? (
        <button type="button" className="mt-1 text-xs text-accent underline" aria-pressed={whole} onClick={() => setWhole(false)}>
          Show only what changed
        </button>
      ) : null}
    </div>
  );
}

type Change = { key: string; entry_id?: string; section?: string; before?: Json; after?: Json };
const isChange = (v: Json): v is Change => isObject(v) && typeof v.key === "string";

function Changes({ changes }: { changes: Change[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <caption className="sr-only">Recorded changes</caption>
        <thead>
          <tr>
            {["Setting", "Before", "After"].map((h) => (
              <th key={h} scope="col" className="pb-1 pr-4 text-xs font-semibold uppercase tracking-label text-muted">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {changes.map((c, i) => (
            <ChangeRow key={`${c.section ?? ""}${c.key}${c.entry_id ?? ""}${i}`} c={c} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ChangeRow({ c }: { c: Change }) {
  const [whole, setWhole] = useState(false);
  return (
    <tr>
      <td className={`${TEXT} border-t border-line py-1.5 pr-4 align-top font-mono text-xs`}>
        {[c.section, c.key].filter(Boolean).join(" · ") + (c.entry_id ? ` › ${c.entry_id}` : "")}
      </td>
      <td className="border-t border-line py-1.5 pr-4 align-top">
        <ChangeCell side="before" before={c.before} after={c.after} whole={whole} setWhole={setWhole} />
      </td>
      <td className="border-t border-line py-1.5 align-top">
        <ChangeCell side="after" before={c.before} after={c.after} whole={whole} setWhole={setWhole} />
      </td>
    </tr>
  );
}

/** The detail under a row: changes, before → after pairs, the rest of `metadata`, then the row's own fields. */
export function AuditDetail({ entry }: { entry: AuditEntry }) {
  const meta = isObject(entry.metadata) ? entry.metadata : {};
  const changes = Array.isArray(meta.change) && meta.change.every(isChange) && meta.change.length > 0 ? (meta.change as Change[]) : null;
  // A top-level `old` and `new`, as `addon_sources` writes them.
  const topPair: [Json, Json] | null = "old" in meta && "new" in meta ? [meta.old, meta.new] : null;
  const pairs: { name: string; before: Json; after: Json }[] = [];
  const rest: [string, Json][] = [];
  for (const [k, v] of Object.entries(meta)) {
    if (k === "change" && changes) continue;
    if ((k === "old" || k === "new") && topPair) continue;
    const p = pairOf(v);
    if (p) pairs.push({ name: label(k), before: p[0], after: p[1] });
    else rest.push([k, v]);
  }
  if (topPair) pairs.push({ name: "value", before: topPair[0], after: topPair[1] });
  const nothing = !changes && pairs.length === 0 && rest.length === 0;
  return (
    <div className="space-y-4 py-1" data-testid="audit-detail">
      {changes ? <Changes changes={changes} /> : null}
      {pairs.length > 0 ? (
        <div className="space-y-1">
          {pairs.map((p) => (
            <BeforeAfter key={p.name} name={p.name} before={p.before} after={p.after} />
          ))}
        </div>
      ) : null}
      {rest.length > 0 ? (
        <dl className="grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]">
          {rest.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-muted">{label(k)}</dt>
              <dd className="min-w-0">
                <Value v={v} />
              </dd>
            </div>
          ))}
        </dl>
      ) : null}
      {nothing ? <p className="text-sm text-muted">No further detail recorded.</p> : null}
      <dl className="grid grid-cols-1 gap-x-4 gap-y-1 border-t border-line pt-3 text-sm sm:grid-cols-[max-content_1fr]">
        <Row k="Source">{entry.source}</Row>
        {entry.ip ? <Row k="IP">{entry.ip}</Row> : null}
        <Row k="Actor">
          {entry.actor_kind}
          {entry.actor_id ? <CopyValue label="Actor id" value={entry.actor_id} /> : null}
        </Row>
        {entry.target_kind ? (
          <Row k="Target">
            {entry.target_kind}
            {entry.target_id ? <CopyValue label="Target id" value={entry.target_id} /> : null}
          </Row>
        ) : null}
        {entry.workspace_id ? (
          <Row k="Workspace">
            <CopyValue label="Workspace id" value={entry.workspace_id} />
          </Row>
        ) : null}
        <Row k="Entry">
          <CopyValue label="Entry id" value={entry.id} />
        </Row>
      </dl>
    </div>
  );
}

function Row({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="contents">
      <dt className="text-muted">{k}</dt>
      <dd className="min-w-0 space-y-1 break-words">{children}</dd>
    </div>
  );
}
