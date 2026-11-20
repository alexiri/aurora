/**
 * (P5-REFINE-18 #7, admin-6) Chat platforms the product names but has no connector type for yet. The list on
 * Administration › Chat platforms shows them as "Not available yet"; a type the API lists is "Available".
 */
export const NOT_AVAILABLE_YET: { id: string; name: string }[] = [
  { id: "slack", name: "Slack" },
  { id: "teams", name: "Microsoft Teams" },
];

/** The not-available rows that the API does not already list under another name (matched by id or name, any case). */
export function notAvailableYet(listed: { type_id: string; name: string }[]): { id: string; name: string }[] {
  const seen = new Set(listed.flatMap((t) => [t.type_id.toLowerCase(), t.name.toLowerCase()]));
  return NOT_AVAILABLE_YET.filter((p) => !seen.has(p.id) && !seen.has(p.name.toLowerCase()));
}
