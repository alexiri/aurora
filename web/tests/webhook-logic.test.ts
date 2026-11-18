import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import type { WebhookPreset } from "@/lib/api/webhook";
import {
  buildFromExample,
  configIssues,
  coveredBy,
  connectRefusal,
  editPick,
  effectiveLimitSentence,
  emptyConfig,
  inferType,
  initialInputs,
  invalidSlot,
  issuesAt,
  normalizePath,
  parseExample,
  parseRate,
  pickAt,
  quickIssues,
  slotNameFrom,
  suggestPreset,
  unmatchedIssues,
  unpickAt,
  valuesFromInputs,
  type Json,
  type Picks,
} from "@/lib/webhook/logic";

// P5-WEBHOOK-4 (C-913): the example picker, preset suggestion, error mapping and the values form: the pure parts.

const example = (text: string): Json => {
  const r = parseExample(text);
  if (!r.ok) throw new Error(r.message);
  return r.value;
};

describe("example picker", () => {
  it("a Slack-style example becomes the template with {{text}} and a string slot", () => {
    const root = example('{"text": "Hello, world"}');
    const picks = pickAt({}, "/text", "text");
    const { template, slots } = buildFromExample(root, picks);
    expect(template).toBe('{\n  "text": {{text}}\n}');
    expect(slots).toEqual([{ name: "text", type: "string", required: true, description: "", example: "Hello, world" }]);
  });

  it("keeps the fixed parts and handles nested objects and arrays", () => {
    const root = example('{"username":"Aurora","attachments":[{"title":"T","tags":["a","b"],"n":3,"ok":true,"w":1.5}]}');
    let picks: Picks = {};
    picks = pickAt(picks, "/attachments/0/title", "title");
    picks = pickAt(picks, "/attachments/0/tags", "tags");
    picks = pickAt(picks, "/attachments/0/n", "n");
    picks = pickAt(picks, "/attachments/0/ok", "ok");
    picks = pickAt(picks, "/attachments/0/w", "w");
    const { template, slots } = buildFromExample(root, picks);
    expect(JSON.parse(template.replace(/\{\{\w+\}\}/g, "0"))).toMatchObject({ username: "Aurora" });
    expect(template).toContain('"username": "Aurora"');
    expect(slots.map((s) => [s.name, s.type])).toEqual([
      ["title", "string"],
      ["tags", "string_list"],
      ["n", "integer"],
      ["ok", "boolean"],
      ["w", "number"],
    ]);
  });

  it("an object becomes a json slot; picking a parent drops the picks beneath it", () => {
    const root = example('{"card":{"a":"x","b":"y"}}');
    let picks = pickAt({}, "/card/a", "a");
    picks = pickAt(picks, "/card", "card");
    expect(Object.keys(picks)).toEqual(["/card"]);
    const { template, slots } = buildFromExample(root, picks);
    expect(template).toBe('{\n  "card": {{card}}\n}');
    expect(slots[0]).toMatchObject({ name: "card", type: "json" });
    expect(coveredBy(picks, "/card/a")).toBe("/card");
    expect(coveredBy(picks, "/card")).toBeNull();
  });

  it("an array that is not all text is a json slot", () => {
    expect(inferType([1, 2])).toBe("json");
    expect(inferType(["a"])).toBe("string_list");
    expect(inferType([])).toBe("string_list");
  });

  it("makes key names valid and unique", () => {
    expect(slotNameFrom("iconEmoji", new Set())).toBe("icon_emoji");
    expect(slotNameFrom("1st-place", new Set())).toBe("v_1st_place");
    expect(slotNameFrom("", new Set())).toBe("value");
    expect(slotNameFrom("A".repeat(60), new Set())).toMatch(/^[a-z][a-z0-9_]{0,39}$/);
    expect(slotNameFrom("text", new Set(["text"]))).toBe("text_2");
    expect(slotNameFrom("text", new Set(["text", "text_2"]))).toBe("text_3");
    const root = example('{"a":{"text":"x"},"b":{"text":"y"}}');
    const picks = pickAt(pickAt({}, "/a/text", "text"), "/b/text", "text");
    expect(buildFromExample(root, picks).slots.map((s) => s.name)).toEqual(["text", "text_2"]);
  });

  it("un-making a slot restores the constant", () => {
    const root = example('{"text":"Hello"}');
    const picked = pickAt({}, "/text", "text");
    expect(buildFromExample(root, picked).template).toContain("{{text}}");
    const back = buildFromExample(root, unpickAt(picked, "/text"));
    expect(back.slots).toEqual([]);
    expect(back.template).toBe('{\n  "text": "Hello"\n}');
  });

  it("carries the edited description, required and limits; a slot at the root is always required", () => {
    const root = example('{"t":"x","l":["a"]}');
    let picks = pickAt(pickAt({}, "/t", "t"), "/l", "l");
    picks = editPick(picks, "/t", { description: "The message", required: false, max_length: 40 });
    picks = editPick(picks, "/l", { max_items: 3 });
    const { slots } = buildFromExample(root, picks);
    expect(slots[0]).toMatchObject({ description: "The message", required: false, max_length: 40 });
    expect(slots[1]).toMatchObject({ max_items: 3 });
    const rootSlots = buildFromExample(example('{"a":1}'), editPick(pickAt({}, "", "payload"), "", { required: false }));
    expect(rootSlots.template).toBe("{{payload}}");
    expect(rootSlots.slots[0]).toMatchObject({ name: "payload", type: "json", required: true });
  });

  it("refuses text that is not JSON, in words", () => {
    const r = parseExample("{nope");
    expect(r.ok).toBe(false);
    expect(parseExample("").ok).toBe(false);
  });
});

describe("preset suggestion by host", () => {
  const preset = (id: string, hosts?: string[]): WebhookPreset => ({ id, name: id, suggest_for_hosts: hosts, secret_value_names: [], config: emptyConfig() });
  const presets = [preset("slack", ["hooks.slack.com"]), preset("discord", ["discord.com", "discordapp.com"]), preset("generic-json")];
  it("suggests the preset whose host list contains the URL's host", () => {
    expect(suggestPreset(presets, "https://hooks.slack.com/services/T0/B0/xyz")?.id).toBe("slack");
    expect(suggestPreset(presets, "https://DISCORDAPP.com/api/webhooks/1/abc")?.id).toBe("discord");
  });
  it("suggests nothing for an unknown host, an empty URL or a partial one", () => {
    expect(suggestPreset(presets, "https://example.org/hook")).toBeNull();
    expect(suggestPreset(presets, "")).toBeNull();
    expect(suggestPreset(presets, "https://")).toBeNull();
  });
});

describe("error-path mapping, one test per C-909 code", () => {
  const err = (path: string, code: string) => new ApiError("webhook_config_invalid", "x", { fields: [{ path, code }] }, 422);
  const cases: [string, string, string][] = [
    ["template_json", "/template", "template"],
    ["template_syntax", "/template", "template"],
    ["placeholder_unknown", "/template", "template"],
    ["slot_unused", "/slots/1", "slots.1"],
    ["secret_unused", "/secret_values/token", "secret_values.token"],
    ["slot_name", "/slots/0/name", "slots.0.name"],
    ["slot_duplicate", "/slots/2/name", "slots.2.name"],
    ["slot_type_content_type", "/slots/0/type", "slots.0.type"],
    ["slot_limits", "/slots/3/max_length", "slots.3.max_length"],
    ["optional_slot_at_root", "/slots/0/required", "slots.0.required"],
    ["schema_unsupported", "/slots/0/schema", "slots.0.schema"],
    ["header_name_denied", "/headers/0/name", "headers.0.name"],
    ["header_placeholder", "/headers/1/value", "headers.1.value"],
    ["rate_above_ceiling", "/rate/per_minute", "rate.per_minute"],
    ["too_large", "/template", "template"],
  ];
  for (const [code, path, key] of cases) {
    it(`${code} lands on ${key}`, () => {
      const issues = configIssues(err(path, code));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({ path: key, code });
      expect(issues[0]!.message.length).toBeGreaterThan(10);
      expect(issuesAt(issues, key.split(".")[0]!)).toHaveLength(1);
      expect(unmatchedIssues(issues)).toEqual([]);
    });
  }
  it("accepts the other spellings of a path", () => {
    expect(normalizePath("webhook/slots/2/name")).toBe("slots.2.name");
    expect(normalizePath("slots[2].name")).toBe("slots.2.name");
    expect(normalizePath("webhook.rate.per_day")).toBe("rate.per_day");
  });
  it("an unknown path goes to the general list; any other error gives nothing", () => {
    expect(unmatchedIssues(configIssues(err("/whatever", "too_large")))).toHaveLength(1);
    expect(configIssues(new ApiError("forbidden", "x", null, 403))).toEqual([]);
    expect(configIssues(new Error("x"))).toEqual([]);
  });
  it("maps invalid_slot and the connect refusals", () => {
    expect(invalidSlot(new ApiError("invalid_slot", "x", { slot: "text", rule: "max_length" }, 422))).toEqual({ slot: "text", message: "This is longer than the limit." });
    expect(invalidSlot(new ApiError("other", "x", null, 422))).toBeNull();
    expect(connectRefusal(new ApiError("link_not_https", "x", null, 422))?.field).toBe("url");
    expect(connectRefusal(new ApiError("link_invalid", "x", null, 422))?.field).toBe("url");
    expect(connectRefusal(new ApiError("link_destination_refused", "x", null, 422))?.field).toBe("url");
    expect(connectRefusal(new ApiError("outgoing_webhooks_disabled", "x", null, 403))?.field).toBeNull();
    expect(connectRefusal(new ApiError("webhook_config_invalid", "x", null, 422))).toBeNull();
  });
});

describe("quick checks", () => {
  it("finds unknown and unused blanks before any request", () => {
    const c = { ...emptyConfig(), template: '{"a": {{text}}, "t": "{{secret:tok}}"}', slots: [{ name: "other", type: "string" as const, required: true, description: "" }] };
    const codes = quickIssues(c, ["tok", "spare"]).map((i) => `${i.path}:${i.code}`);
    expect(codes).toContain("template:placeholder_unknown");
    expect(codes).toContain("slots.0:slot_unused");
    expect(codes).toContain("secret_values.spare:secret_unused");
    expect(codes.some((x) => x.includes("secret_values.tok"))).toBe(false);
  });
  it("flags a bad or duplicate name, and a slot placeholder in a header", () => {
    const s = (name: string) => ({ name, type: "string" as const, required: true, description: "" });
    const c = { ...emptyConfig(), template: "{{a}}{{a}}", slots: [s("a"), s("a"), s("Bad")], headers: [{ name: "X-Key", value: "{{a}}" }] };
    const codes = quickIssues(c, []).map((i) => `${i.path}:${i.code}`);
    expect(codes).toContain("slots.1.name:slot_duplicate");
    expect(codes).toContain("slots.2.name:slot_name");
    expect(codes).toContain("headers.0.value:header_placeholder");
  });
  it("a secret used only in a header counts as used", () => {
    const c = { ...emptyConfig(), template: "", headers: [{ name: "Authorization", value: "Bearer {{secret:tok}}" }] };
    expect(quickIssues(c, ["tok"])).toEqual([]);
  });
});

describe("the values form", () => {
  const slots = [
    { name: "text", type: "string" as const, required: true, description: "", example: "Hi" },
    { name: "n", type: "integer" as const, required: false, description: "", example: 3 },
    { name: "tags", type: "string_list" as const, required: false, description: "", example: ["a", "b"] },
    { name: "flag", type: "boolean" as const, required: false, description: "" },
    { name: "extra", type: "json" as const, required: false, description: "", example: { a: 1 } },
  ];
  it("starts from the examples", () => {
    expect(initialInputs(slots)).toEqual({ text: "Hi", n: "3", tags: "a\nb", flag: "false", extra: '{\n  "a": 1\n}' });
  });
  it("converts inputs to typed values and leaves out a blank optional one", () => {
    const { values, errors } = valuesFromInputs(slots, { text: "Hello", n: "", tags: "x\n\ny", flag: "true", extra: '{"b":2}' });
    expect(errors).toEqual({});
    expect(values).toEqual({ text: "Hello", tags: ["x", "y"], flag: true, extra: { b: 2 } });
  });
  it("names a bad number, a bad JSON and a missing required one", () => {
    const { errors } = valuesFromInputs([...slots, { name: "must", type: "number", required: true, description: "" }], { text: "x", n: "2.5", tags: "", flag: "false", extra: "{", must: "" });
    expect(Object.keys(errors).sort()).toEqual(["extra", "must", "n"]);
  });
});

describe("limits", () => {
  it("parses the limit fields", () => {
    expect(parseRate("")).toBeNull();
    expect(parseRate(" 12 ")).toBe(12);
    expect(parseRate("0")).toBe("invalid");
    expect(parseRate("1.5")).toBe("invalid");
  });
  it("says the effective limit with its source", () => {
    expect(effectiveLimitSentence({ per_minute: 30, per_day: 500, source: "org" })).toBe("30 a minute, 500 a day (your organization's limit)");
  });
});
