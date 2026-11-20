import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PolicyPresets } from "@/components/policy/policy-presets";
import { applyPreset, detectPreset, PRESET_IDS, PRESET_KEYS, PRESETS, type PresetKey } from "@/lib/policy-presets";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-18 #2 (D-410): the preset mapping in both directions, and the card on Administration.

const DEFAULTS: Record<PresetKey, unknown> = {
  "governance.egress.minimum_posture": "open",
  "governance.egress.open_lane_minimum": "enabled",
  "governance.llm.minimum_posture": "open",
  "governance.llm.allowed_key_scopes": ["organization", "workspace"],
  "byo.enabled": true,
  "byo.minimum_posture": "owners_only",
};

/** The registry's default of a key, read from `aurora-config/src/settings.rs`. */
function registryDefault(key: string): unknown {
  const src = readFileSync(join(__dirname, "../../crates/aurora-config/src/settings.rs"), "utf8");
  const at = src.indexOf(`"${key}"`);
  const m = /json!\(([^)]*)\)/.exec(src.slice(at));
  return JSON.parse(m![1]!);
}

type Row = { key: string; value: unknown; locked?: boolean; source?: string };
const rows = (over: Record<string, unknown> = {}, locked: string[] = []): Row[] =>
  PRESET_KEYS.map((k) => ({ key: k, value: k in over ? over[k] : DEFAULTS[k], locked: locked.includes(k), source: "default" }));

describe("PRESETS", () => {
  it("Open equals a fresh install's defaults, key by key, as the registry has them", () => {
    for (const k of PRESET_KEYS) expect(PRESETS.open.values[k], k).toEqual(registryDefault(k));
  });

  it("detects each preset from its own values, in both directions", () => {
    for (const id of PRESET_IDS) {
      const s = detectPreset(rows(PRESETS[id].values));
      expect(s.preset, id).toBe(id);
      const { values } = applyPreset(id, {}, rows());
      for (const k of PRESET_KEYS) expect(values[k], `${id} ${k}`).toEqual(PRESETS[id].values[k]);
    }
  });

  it("a hand edit is Custom, based on the closest preset", () => {
    const s = detectPreset(rows({ "byo.minimum_posture": "enabled" }));
    expect(s).toMatchObject({ preset: null, basedOn: "open" });
    const near = detectPreset(rows({ ...PRESETS.locked_down.values, "byo.enabled": true }));
    expect(near).toMatchObject({ preset: null, basedOn: "locked_down" });
  });

  it("a section with one Helm-locked key and no other preset key still reads as a preset with the lock, never Custom (CI send-back)", () => {
    // The mock of the Playwright spec: only `governance.egress.minimum_posture` is present, and Helm holds it.
    const s = detectPreset([{ key: "governance.egress.minimum_posture", value: "restricted", locked: true }]);
    expect(s.preset).toBe("open");
    expect(s.lockedApart).toEqual(["governance.egress.minimum_posture"]);
    // Keys the section lacks are skipped, not mismatched: one unlocked key that differs still makes it Custom.
    const custom = detectPreset([
      { key: "governance.egress.minimum_posture", value: "restricted", locked: true },
      { key: "byo.minimum_posture", value: "enabled", locked: false },
    ]);
    expect(custom.preset).toBeNull();
    expect(custom.basedOn).toBe("open");
  });

  it("a Helm-locked value is left out of the match and counted when it keeps the preset from applying", () => {
    const s = detectPreset(rows({ "governance.llm.minimum_posture": "sanctioned_only", "byo.enabled": false }, ["governance.llm.minimum_posture", "byo.enabled"]));
    expect(s.preset).toBe("open");
    expect(s.lockedApart).toEqual(["governance.llm.minimum_posture", "byo.enabled"]);
    // A locked value that already agrees is not mentioned.
    expect(detectPreset(rows({}, ["byo.enabled"])).lockedApart).toEqual([]);
  });

  it("applying keeps the dashboard values it does not own, and never sends a locked key", () => {
    const current = { "governance.egress.baseline_allowlist": [{ host: "docs.example.org" }], "byo.enabled": true };
    const { values, skipped } = applyPreset("locked_down", current, rows({}, ["byo.enabled"]));
    expect(values["governance.egress.baseline_allowlist"]).toEqual([{ host: "docs.example.org" }]);
    expect("byo.enabled" in values).toBe(false);
    expect(skipped).toEqual(["byo.enabled"]);
    expect(values["governance.egress.minimum_posture"]).toBe("credentialed_only");
  });
});

const section = (over: Record<string, unknown> = {}, locked: string[] = [], generation = 4) => ({
  section: "governance",
  title: "Governance",
  description: "",
  editable: true,
  generation,
  fields: [],
  settings: PRESET_KEYS.map((k) => ({
    key: k,
    value: k in over ? over[k] : DEFAULTS[k],
    default: DEFAULTS[k],
    source: k in over ? "dashboard" : "default",
    locked: locked.includes(k),
  })),
});

describe("PolicyPresets card", () => {
  it("shows Open on a fresh install, and choosing Locked down writes the mapped values", async () => {
    let put: { generation: number; values: Record<string, unknown> } | null = null;
    mockFetch({
      // The server keeps what was saved.
      "GET /v1/admin/settings/governance": () => json(200, put ? section(PRESETS.locked_down.values, [], 5) : section()),
      "POST /v1/admin/settings/governance/preview": () => json(200, { generation: 4, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] }),
      "PUT /v1/admin/settings/governance": async (req) => {
        put = await req.json();
        return json(200, section(PRESETS.locked_down.values, [], 5));
      },
    });
    wrap(<PolicyPresets />);
    expect(await screen.findByTestId("preset-state")).toHaveTextContent("Open");
    expect(screen.getByRole("radio", { name: /Open/ })).toBeChecked();
    expect(screen.getByText("Agents reach any website; people may add their own AI keys; owners may run agents elsewhere.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /Locked down/ }));
    await waitFor(() => expect(put).not.toBeNull());
    expect(put!.generation).toBe(4);
    for (const k of PRESET_KEYS) expect(put!.values[k], k).toEqual(PRESETS.locked_down.values[k]);
    await waitFor(() => expect(screen.getByTestId("preset-state")).toHaveTextContent("Locked down"));
  });

  it("asks in sentences before it loosens, and writes nothing until the answer is Yes", async () => {
    const puts = vi.fn();
    mockFetch({
      "GET /v1/admin/settings/governance": () => json(200, section(PRESETS.locked_down.values)),
      "POST /v1/admin/settings/governance/preview": () =>
        json(200, {
          generation: 4,
          diff: [{ key: "byo.enabled", before: false, after: true, before_source: "dashboard", after_source: "dashboard" }],
          loosening: [{ key: "byo.enabled", entry_id: null, reason: "`byo.enabled` becomes less strict." }],
          affected_orgs: [],
          affected_orgs_total: 0,
          warnings: [],
        }),
      "PUT /v1/admin/settings/governance": () => {
        puts();
        return json(200, section());
      },
    });
    wrap(<PolicyPresets />);
    await waitFor(() => expect(screen.getByTestId("preset-state")).toHaveTextContent("Locked down"));
    fireEvent.click(screen.getByRole("radio", { name: /Open/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Allow more than before?" })).toBeInTheDocument();
    expect(within(dialog).getByTestId("loosening-sentences")).toHaveTextContent("People may run agents outside Aurora.");
    expect(puts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Yes, save" }));
    await waitFor(() => expect(puts).toHaveBeenCalledTimes(1));
  });

  it("reads Custom · based on Open after a hand edit, and (n settings set in Helm) with a lock pill when Helm keeps a value", async () => {
    mockFetch({ "GET /v1/admin/settings/governance": () => json(200, section({ "byo.minimum_posture": "enabled" })) });
    const a = wrap(<PolicyPresets />);
    await waitFor(() => expect(screen.getByTestId("preset-state")).toHaveTextContent("Custom · based on Open"));
    expect(screen.getAllByRole("radio").every((r) => !(r as HTMLInputElement).checked)).toBe(true);
    a.unmount();
    mockFetch({ "GET /v1/admin/settings/governance": () => json(200, section({ "governance.llm.minimum_posture": "sanctioned_only", "byo.enabled": false }, ["governance.llm.minimum_posture", "byo.enabled"])) });
    wrap(<PolicyPresets />);
    await waitFor(() => expect(screen.getByTestId("preset-state")).toHaveTextContent("Open (2 settings set in Helm)"));
    expect(screen.getByRole("link", { name: /Set in Helm/ })).toHaveAttribute("href", "/admin/set-in-helm");
  });

  it("keeps today's fields behind Customise, open when the values are custom", async () => {
    mockFetch({ "GET /v1/admin/settings/governance": () => json(200, section()) });
    wrap(
      <PolicyPresets>
        <p>the fields</p>
      </PolicyPresets>,
    );
    await screen.findByTestId("preset-state");
    expect(screen.queryByText("the fields")).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: /Customise/ }));
    expect(screen.getByText("the fields")).toBeInTheDocument();
  });
});
