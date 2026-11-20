import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { describeChange } from "@/lib/settings-words";

// P5-REFINE-18 #3 (admin-3, D-341): a confirm says each change as a sentence and never prints a setting key.

/** The loosening keys of the registry (`aurora-config/src/settings.rs`): a definition followed by a `.loosening(…)` that is not None. */
function looseningKeys(): string[] {
  const src = readFileSync(join(__dirname, "../../crates/aurora-config/src/settings.rs"), "utf8");
  const keys = new Set<string>();
  for (const chunk of src.split("SettingDef::new(").slice(1)) {
    const body = chunk.split(/\n\s*SettingDef::new\(/)[0]!;
    const key = /"([a-z_0-9]+(?:\.[a-z_0-9]+)+)"/.exec(body)?.[1];
    if (key && /\.loosening\(\s*Loosening::(?!None)/.test(body)) keys.add(key);
  }
  return [...keys];
}

describe("describeChange", () => {
  it("walks the registry's loosening keys: every one reads as a sentence with no key and no snake_case", () => {
    const keys = looseningKeys();
    expect(keys.length).toBeGreaterThan(20);
    for (const key of keys) {
      for (const [b, a] of [
        [1, 2],
        [false, true],
        [["a"], ["a", "b"]],
        ["strict", "open"],
      ] as const) {
        const s = describeChange(key, b, a);
        expect(s, key).toMatch(/\.$/);
        expect(s, key).not.toContain(key);
        expect(s, key).not.toMatch(/[a-z]+_[a-z]+/);
        expect(s.length, key).toBeGreaterThan(10);
      }
    }
  });

  it("says the allow-list hosts that were added and removed", () => {
    expect(describeChange("governance.egress.baseline_allowlist", [{ host: "a.example.org" }], [{ host: "a.example.org" }, { host: "docs.example.org" }])).toBe(
      "Agents everywhere may now reach docs.example.org.",
    );
    expect(describeChange("governance.egress.baseline_allowlist", [{ host: "a.example.org" }], [])).toBe("Agents everywhere no longer reach a.example.org by default.");
  });

  it("says retention in days, before and after", () => {
    expect(describeChange("retention.request_log_days", 30, 45)).toBe("Request activity is kept 45 days instead of 30 days.");
  });

  it("says postures and the outside-agents switch in words", () => {
    expect(describeChange("governance.egress.minimum_posture", "credentialed_only", "open")).toBe("Agents may reach any website, even with no account for it.");
    expect(describeChange("byo.enabled", false, true)).toBe("People may run agents outside Aurora.");
    expect(describeChange("governance.llm.allowed_key_scopes", ["organization"], ["organization", "workspace"])).toBe("AI keys may now be added at the workspace level.");
  });

  it("falls back to the field's label, else to words made from the key, and names a keyed entry", () => {
    const field = { key: "x.y_z", label: "Mail limit" } as never;
    expect(describeChange("x.y_z", 5, 9, { field })).toBe("Mail limit changes from 5 to 9.");
    expect(describeChange("x.mail_limit", 5, 9)).toBe("Mail limit changes from 5 to 9.");
    expect(describeChange("x.mail_limit", undefined, undefined)).toBe("Mail limit now allows more than before.");
    expect(describeChange("auth.oidc_providers", undefined, undefined, { entryId: "corp" })).toBe("Oidc providers: corp now allows more than before.");
  });
});
