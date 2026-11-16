/**
 * Test fixtures for C-972: give a hand-built C-772 section its `fields` (one per setting, in order) and
 * `editable: true`. The words are plain test words ("Words of hosted.idle_stop_secs"), not the registry's: the
 * registry's own words are tested in Rust. An enumeration gets one choice per schema option.
 */
type Json = Record<string, unknown>;

function kindOf(s: Json): string {
  const schema = s.schema as { kind: string };
  if (s.secret) return "secret";
  if (s.list === "keyed") return "keyed_list";
  switch (schema.kind) {
    case "bool":
    case "int":
    case "float":
    case "enum":
    case "list":
      return schema.kind;
    case "duration_secs":
      return "duration";
    default:
      return "text";
  }
}

export function fieldFor(s: Json): Json {
  const schema = s.schema as { options?: string[] | null };
  const kind = kindOf(s);
  return {
    key: s.key,
    label: `Words of ${String(s.key)}`,
    help: `Help for ${String(s.key)}.`,
    kind,
    choices:
      kind === "enum"
        ? (schema.options ?? []).map((o) => ({ value: o, label: `Choice ${o}`, description: `What ${o} means.` }))
        : null,
    default_display: "Default words",
    unit: null,
    doc: null,
  };
}

export function withFields<T extends Json>(section: T): T & { fields: Json[]; editable: boolean; current: null } {
  const settings = (section.settings ?? []) as Json[];
  return { editable: true, current: null, fields: settings.map(fieldFor), ...section };
}
