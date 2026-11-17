import { Badge } from "@/components/ui/badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import type { components } from "@/lib/api/schema";

type Usage = components["schemas"]["Usage"];

const n = (v: number | null | undefined): string => (v ?? 0).toLocaleString("en-US");

/** The marker of a turn whose harness reported models the turn could not have used (C-879, SEC-123). */
export const UNVERIFIED_TEXT = "model reported by the harness could not be verified";

/**
 * (phase 5, P5-MODELS-3, D-304, C-879) A turn's tokens per model, subagents included. A turn recorded before per-model
 * usage has no `by_model`: its totals count under `model`. A turn with `unverified_models` carries the marker, because
 * the tokens of an unknown model were folded into the default model's row.
 */
export function TurnUsage({ usage }: { usage: Usage }) {
  const rows =
    usage.by_model && usage.by_model.length > 0
      ? usage.by_model
      : [
          {
            model: usage.model ?? "unknown model",
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens ?? 0,
            cache_write_tokens: usage.cache_write_tokens ?? 0,
          },
        ];
  return (
    <div className="space-y-2" data-testid="turn-usage">
      <DataTable caption="Tokens per model">
        <thead>
          <tr>
            <Th>Model</Th>
            <Th className="text-right">Input</Th>
            <Th className="text-right">Output</Th>
            <Th className="text-right">Cache read</Th>
            <Th className="text-right">Cache write</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.model}>
              <Td className="break-all font-mono text-xs">{r.model}</Td>
              <Td className="text-right tabular-nums">{n(r.input_tokens)}</Td>
              <Td className="text-right tabular-nums">{n(r.output_tokens)}</Td>
              <Td className="text-right tabular-nums">{n(r.cache_read_tokens)}</Td>
              <Td className="text-right tabular-nums">{n(r.cache_write_tokens)}</Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      {usage.unverified_models ? (
        <p className="flex flex-wrap items-center gap-2 text-xs text-ink-2" data-testid="unverified-models">
          <Badge tone="pending">Unverified</Badge>
          {UNVERIFIED_TEXT}
        </p>
      ) : null}
    </div>
  );
}
