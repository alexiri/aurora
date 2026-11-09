"use client";

import { Badge } from "@/components/ui/badge";
import { formFields } from "@/lib/api/connectors";

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (typeof v === "object" && v !== null ? (v as Obj) : {});

const enabledCaps = (m: Obj) =>
  new Map(Object.entries(obj(m.capabilities)).filter(([, v]) => v === true || (typeof v === "object" && v !== null)));

function keyedFields(m: Obj): Map<string, unknown> {
  const out = new Map<string, unknown>();
  for (const f of formFields(m.integration_setup)) out.set(`Integration: ${f.name}`, f);
  for (const f of formFields(obj(obj(m.presence_setup).dedicated))) out.set(`Dedicated presence: ${f.name}`, f);
  for (const f of formFields(m.instance_setup)) out.set(`Connector setting: ${f.name}`, f);
  return out;
}

export type Change = { added: string[]; removed: string[]; changed: string[] };

function diffMaps(a: Map<string, unknown>, b: Map<string, unknown>): Change {
  const out: Change = { added: [], removed: [], changed: [] };
  for (const [k, v] of b) {
    if (!a.has(k)) out.added.push(k);
    else if (JSON.stringify(a.get(k)) !== JSON.stringify(v)) out.changed.push(k);
  }
  for (const k of a.keys()) if (!b.has(k)) out.removed.push(k);
  return out;
}

/** Old-vs-new manifest differences a reviewer must see (docs/14 manifest approval). */
export function diffManifest(approved: unknown, next: unknown) {
  const a = obj(approved);
  const b = obj(next);
  const cb = (m: Obj) => new Map((Array.isArray(m.public_callbacks) ? m.public_callbacks : []).map((c) => [String(c), c]));
  return {
    capabilities: diffMaps(enabledCaps(a), enabledCaps(b)),
    fields: diffMaps(keyedFields(a), keyedFields(b)),
    callbacks: diffMaps(cb(a), cb(b)),
  };
}

function ChangeList({ label, change }: { label: string; change: Change }) {
  const rows: [string, string, string[]][] = [
    ["Added", "text-danger-ink", change.added],
    ["Removed", "text-accent-ink", change.removed],
    ["Changed", "text-pending-ink", change.changed],
  ];
  const any = rows.some(([, , l]) => l.length);
  return (
    <div>
      <h5 className="font-medium text-ink">{label}</h5>
      {any ? (
        <ul className="list-disc pl-5">
          {rows.flatMap(([word, cls, list]) =>
            list.map((k) => (
              <li key={`${word}-${k}`} className={cls}>
                {word}: <code className="font-mono">{k}</code>
              </li>
            )),
          )}
        </ul>
      ) : (
        <p className="text-muted">No changes.</p>
      )}
    </div>
  );
}

/** Differences between the last approved manifest and the pending one. */
export function ManifestDiff({ approved, manifest }: { approved: unknown; manifest: unknown }) {
  const d = diffManifest(approved, manifest);
  return (
    <div className="space-y-2 text-sm" data-testid="manifest-diff">
      <h4 className="font-medium text-ink">Changes since the approved manifest</h4>
      <ChangeList label="Capabilities" change={d.capabilities} />
      <ChangeList label="Setup fields" change={d.fields} />
      <ChangeList label="Public callbacks" change={d.callbacks} />
    </div>
  );
}

/** Capabilities, public callbacks and setup fields announced by a manifest, for the reviewer. */
export function ManifestSummary({ manifest }: { manifest: unknown }) {
  const m = obj(manifest);
  const caps = Object.entries(obj(m.capabilities)).filter(([, v]) => v === true).map(([k]) => k);
  const callbacks = (Array.isArray(m.public_callbacks) ? m.public_callbacks : []).map(String);
  const integ = formFields(m.integration_setup);
  const pres = formFields(obj(obj(m.presence_setup).dedicated));
  const inst = formFields(m.instance_setup);
  return (
    <div className="space-y-2 text-sm" data-testid="manifest-summary">
      <div>
        <h4 className="font-medium text-ink">Capabilities</h4>
        {caps.length ? (
          <ul className="mt-1 flex flex-wrap gap-1">
            {caps.map((c) => (
              <li key={c}>
                <Badge className="font-mono normal-case tracking-normal">{c}</Badge>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted">None.</p>
        )}
      </div>
      <div>
        <h4 className="font-medium text-ink">Public callbacks</h4>
        {callbacks.length ? (
          <ul className="list-disc pl-5">
            {callbacks.map((c) => (
              <li key={c}>
                <code className="font-mono">{c}</code>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted">None.</p>
        )}
      </div>
      <div>
        <h4 className="font-medium text-ink">Setup fields</h4>
        {integ.length + pres.length + inst.length ? (
          <ul className="list-disc pl-5">
            {inst.map((f) => (
              <li key={`s-${f.name}`}>
                Connector setting: <code className="font-mono">{f.name}</code> ({f.type})
              </li>
            ))}
            {integ.map((f) => (
              <li key={`i-${f.name}`}>
                Integration: <code className="font-mono">{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
            {pres.map((f) => (
              <li key={`p-${f.name}`}>
                Dedicated presence: <code className="font-mono">{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted">None.</p>
        )}
      </div>
    </div>
  );
}
