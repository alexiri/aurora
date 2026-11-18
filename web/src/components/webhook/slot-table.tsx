"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import type { ContentType, Slot, SlotType } from "@/lib/api/webhook";
import { firstMessage, LIMITS, SLOT_TYPES, type Issue } from "@/lib/webhook/logic";
import { cn } from "@/lib/utils";

// The slot table of the raw editor (C-913): add, remove, reorder, type, limits, enum values and a schema for `json`.
// A card per slot rather than a table row, so it works by touch at 390 px.

function intOrUndefined(v: string, max: number): number | undefined {
  const n = Number(v);
  return v.trim() === "" || !Number.isInteger(n) || n < 1 ? undefined : Math.min(n, max);
}

/** The schema as text while it is being typed; it is committed only when it parses as an object. */
function SchemaField({ id, slot, onChange, error }: { id: string; slot: Slot; onChange: (schema: Record<string, unknown> | undefined) => void; error?: string }) {
  const [text, setText] = useState(slot.schema ? JSON.stringify(slot.schema, null, 2) : "");
  const [bad, setBad] = useState(false);
  return (
    <Field
      id={id}
      label="Schema (optional)"
      hint="Limits what the agent may send: type, properties, required, items, enum, maxLength, minimum, maximum, additionalProperties."
      error={bad ? "That is not a JSON object." : error}
    >
      <textarea
        id={id}
        className={cn(inputClass, "min-h-24 font-mono text-xs")}
        spellCheck={false}
        value={text}
        onChange={(e) => {
          const t = e.target.value;
          setText(t);
          if (t.trim() === "") {
            setBad(false);
            return onChange(undefined);
          }
          try {
            const v: unknown = JSON.parse(t);
            if (typeof v === "object" && v !== null && !Array.isArray(v)) {
              setBad(false);
              onChange(v as Record<string, unknown>);
            } else setBad(true);
          } catch {
            setBad(true);
          }
        }}
      />
    </Field>
  );
}

function SlotCard({
  slot,
  index,
  count,
  contentType,
  issues,
  onChange,
  onMove,
  onRemove,
}: {
  slot: Slot;
  index: number;
  count: number;
  contentType: ContentType;
  issues: Issue[];
  onChange: (s: Slot) => void;
  onMove: (delta: -1 | 1) => void;
  onRemove: () => void;
}) {
  const p = `slots.${index}`;
  const id = `slot-${index}`;
  const set = (patch: Partial<Slot>) => onChange({ ...slot, ...patch });
  const own = issues.filter((i) => i.path === p);
  const jsonNotAllowed = slot.type === "json" && contentType !== "application/json";
  return (
    <li className="space-y-3 rounded-control border border-line p-3" aria-label={`Value ${slot.name || index + 1}`}>
      {own.map((i) => (
        <p key={i.code} role="alert" className="text-xs text-danger-ink">
          {i.message}
        </p>
      ))}
      <Field id={`${id}-name`} label="Name" error={firstMessage(issues, `${p}.name`)}>
        <input id={`${id}-name`} className={inputClass} spellCheck={false} value={slot.name} onChange={(e) => set({ name: e.target.value })} />
      </Field>
      <Field id={`${id}-type`} label="Kind of value" error={firstMessage(issues, `${p}.type`) ?? (jsonNotAllowed ? "Any JSON needs the JSON content type." : undefined)}>
        <select
          id={`${id}-type`}
          className={inputClass}
          value={slot.type}
          onChange={(e) => {
            const type = e.target.value as SlotType;
            const next: Slot = { ...slot, type };
            // Limits and lists belong to one type each.
            if (type !== "string" && type !== "string_list") delete next.max_length;
            if (type !== "string_list") delete next.max_items;
            if (type !== "enum") delete next.enum;
            else next.enum = slot.enum?.length ? slot.enum : [""];
            if (type !== "json") delete next.schema;
            onChange(next);
          }}
        >
          {SLOT_TYPES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
      </Field>
      <Field id={`${id}-desc`} label="What the agent should put here (optional)" hint="Up to 300 characters. The agent reads it." error={firstMessage(issues, `${p}.description`)}>
        <input id={`${id}-desc`} className={inputClass} maxLength={LIMITS.slotDescription} value={slot.description} onChange={(e) => set({ description: e.target.value })} />
      </Field>
      <CheckboxField id={`${id}-req`} label="Required" hint={firstMessage(issues, `${p}.required`)} checked={slot.required} onChange={(v) => set({ required: v })} />
      {slot.type === "string" || slot.type === "string_list" ? (
        <Field id={`${id}-max`} label="Longest text (optional)" error={firstMessage(issues, `${p}.max_length`)}>
          <input id={`${id}-max`} className={inputClass} inputMode="numeric" value={slot.max_length ?? ""} onChange={(e) => set({ max_length: intOrUndefined(e.target.value, 65536) })} />
        </Field>
      ) : null}
      {slot.type === "string_list" ? (
        <Field id={`${id}-items`} label="Most items (optional)" error={firstMessage(issues, `${p}.max_items`)}>
          <input id={`${id}-items`} className={inputClass} inputMode="numeric" value={slot.max_items ?? ""} onChange={(e) => set({ max_items: intOrUndefined(e.target.value, 100) })} />
        </Field>
      ) : null}
      {slot.type === "enum" ? (
        <Field id={`${id}-enum`} label="Allowed values" hint="One per line, up to 50." error={firstMessage(issues, `${p}.enum`)}>
          <textarea
            id={`${id}-enum`}
            className={cn(inputClass, "min-h-20")}
            value={(slot.enum ?? []).join("\n")}
            onChange={(e) => set({ enum: e.target.value.split("\n") })}
          />
        </Field>
      ) : null}
      {slot.type === "json" ? <SchemaField id={`${id}-schema`} slot={slot} onChange={(schema) => {
        const next: Slot = { ...slot };
        if (schema) next.schema = schema;
        else delete next.schema;
        onChange(next);
      }} error={firstMessage(issues, `${p}.schema`)} /> : null}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" disabled={index === 0} onClick={() => onMove(-1)} aria-label={`Move up ${slot.name || "value"}`}>
          Move up
        </Button>
        <Button type="button" variant="outline" size="sm" disabled={index === count - 1} onClick={() => onMove(1)} aria-label={`Move down ${slot.name || "value"}`}>
          Move down
        </Button>
        <Button type="button" variant="danger" size="sm" onClick={onRemove} aria-label={`Remove ${slot.name || "value"}`}>
          Remove
        </Button>
      </div>
    </li>
  );
}

export function SlotTable({
  slots,
  contentType,
  issues,
  onChange,
}: {
  slots: Slot[];
  contentType: ContentType;
  issues: Issue[];
  onChange: (slots: Slot[]) => void;
}) {
  return (
    <div className="space-y-3">
      {slots.length === 0 ? <p className="text-sm text-ink-2">No values yet. Add one for each blank the agent fills in.</p> : null}
      {slots.length > 0 ? (
        <ul className="space-y-3" aria-label="Values the agent fills in">
          {slots.map((s, i) => (
            <SlotCard
              // The index is the identity: names change as they are typed.
              key={i}
              slot={s}
              index={i}
              count={slots.length}
              contentType={contentType}
              issues={issues}
              onChange={(n) => onChange(slots.map((x, j) => (j === i ? n : x)))}
              onMove={(d) => {
                const next = [...slots];
                const [m] = next.splice(i, 1);
                next.splice(i + d, 0, m!);
                onChange(next);
              }}
              onRemove={() => onChange(slots.filter((_, j) => j !== i))}
            />
          ))}
        </ul>
      ) : null}
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={slots.length >= LIMITS.slots}
        onClick={() => onChange([...slots, { name: `value_${slots.length + 1}`, type: "string", required: true, description: "" }])}
      >
        Add a value
      </Button>
      {slots.length >= LIMITS.slots ? <p className="text-xs text-muted">Up to {LIMITS.slots} values.</p> : null}
    </div>
  );
}
