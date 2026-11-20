import { describe, expect, it } from "vitest";

import type { FieldInfo } from "@/lib/api/settings";
import { fieldErrorWords, settingKeyOfPath, technicalNames } from "@/lib/settings-words";

const minutes: FieldInfo = {
  key: "skills.library.refresh_interval_secs",
  label: "Skill library refresh",
  help: "",
  kind: "duration",
  unit: "minutes",
  choices: null,
  default_display: "1 hour",
  doc: null,
};

describe("field errors in the field's own words (P5-REFINE-18 #4, admin-4)", () => {
  it("turns the server's seconds into the minutes field's unit and drops the env name", () => {
    const m = "must be at least 300 (LIBRARY_REFRESH_INTERVAL has a 5 minute floor)";
    const words = fieldErrorWords("skills.library.refresh_interval_secs", m, minutes);
    expect(words).toBe("Enter 5 minutes or more. Aurora checks at most once every 5 minutes.");
    expect(words).not.toMatch(/LIBRARY|_secs|300/);
    expect(technicalNames("skills.library.refresh_interval_secs", m)).toEqual(expect.arrayContaining(["LIBRARY_REFRESH_INTERVAL", "skills.library.refresh_interval_secs"]));
  });

  it("says a private range isn't valid and shows an example", () => {
    expect(fieldErrorWords("gateway.allowed_private[0]", "`10.0.0.0/33`: prefix too long")).toBe("Row 1: That isn't a valid address range. Use a form like 10.0.0.0/24.");
  });

  it("names the row of an allow-list entry", () => {
    expect(fieldErrorWords("governance.egress.baseline_allowlist[0].host", "not a valid host pattern")).toBe("Row 1: that isn't a website name. Try docs.example.org.");
    expect(fieldErrorWords("governance.egress.baseline_allowlist[2].path", "must start with `/`")).toBe("Row 3: the path must start with a slash, like /docs.");
  });

  it("states a range in the field's unit and never leaves a dotted key behind", () => {
    const f: FieldInfo = { ...minutes, key: "governance.agent_tokens.max_age_days", label: "Agent token lifetime", unit: "days" };
    expect(fieldErrorWords("governance.agent_tokens.max_age_days", "0 is out of range; expected 1..=365 days (or unset for no limit)", f)).toBe("Enter a number from 1 to 365 days.");
    expect(fieldErrorWords("a.b.c", "`a.b.c` something odd with x.y.z here", { ...minutes, label: "Odd one" })).not.toMatch(/a\.b\.c|x\.y\.z/);
  });

  it("maps a list item's pointer to the setting", () => {
    expect(settingKeyOfPath("governance.egress.baseline_allowlist[0].host")).toBe("governance.egress.baseline_allowlist");
    expect(settingKeyOfPath("llm.endpoints/team/base_url")).toBe("llm.endpoints");
  });
});
