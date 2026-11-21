import { describe, expect, it } from "vitest";

import { MAX_FILES, MAX_ZIP_BYTES, nameProblem, readZip, writeZip, ZipError, crc32 } from "@/lib/export/zip";
import { parseYaml, stringifyYaml, YamlError, type YamlValue } from "@/lib/export/yaml";

// P5-REFINE-19: the agent export's two file formats. No dependency: a store-only zip and a restricted YAML.

const enc = (s: string) => new TextEncoder().encode(s);
const text = (b: Uint8Array) => new TextDecoder().decode(b);

describe("zip", () => {
  it("round-trips files, in order, with UTF-8 names and content", async () => {
    const bytes = writeZip([
      { name: "daneel/agent.yaml", data: enc("format: 1\n") },
      { name: "daneel/memory/café.md", data: enc("héllo ✓") },
      { name: "daneel/empty.md", data: new Uint8Array() },
    ]);
    const back = await readZip(bytes);
    expect(back.map((e) => e.name)).toEqual(["daneel/agent.yaml", "daneel/memory/café.md", "daneel/empty.md"]);
    expect(text(back[1]!.data)).toBe("héllo ✓");
    expect(back[2]!.data.length).toBe(0);
  });

  it("computes the standard CRC-32", () => {
    expect(crc32(enc("123456789")).toString(16)).toBe("cbf43926");
  });

  it("refuses names that are absolute, hidden or climb out, when writing and when reading", async () => {
    for (const bad of ["/etc/passwd", "a/../b", "../x", ".hidden/x", "a/.env", "a\\b", "C:/x"]) {
      expect(nameProblem(bad), bad).not.toBeNull();
      expect(() => writeZip([{ name: bad, data: enc("x") }]), bad).toThrow(ZipError);
    }
    // A hand-made archive with a bad name is refused by the reader.
    const sneaky = writeZip([{ name: "ok/x.md", data: enc("x") }]);
    const patched = new Uint8Array(sneaky);
    const name = enc("ok/x.md");
    const swap = enc("../x.md");
    for (let i = 0; i + name.length <= patched.length; i++) {
      if (name.every((b, k) => patched[i + k] === b)) patched.set(swap.subarray(0, name.length), i);
    }
    await expect(readZip(patched)).rejects.toThrow(/climbs out/);
  });

  it("refuses something that is not a zip, a damaged one, and one that is too big", async () => {
    await expect(readZip(enc("not a zip at all, really not"))).rejects.toThrow(/not a zip/);
    // A fixed time: the headers carry the DOS date and time, and on some days one of those bytes was 104 (the "h" the
    // old flip looked for), so the flip hit a timestamp, the CRC still matched and the zip read fine.
    const good = writeZip([{ name: "a/x.md", data: enc("hello") }], new Date(2026, 0, 2, 3, 4, 6));
    const damaged = new Uint8Array(good);
    // Flip a byte inside the stored data: the first "hello" after the local header's file name.
    const at = good.findIndex((_, i) => new TextDecoder().decode(good.subarray(i, i + 5)) === "hello");
    expect(at).toBeGreaterThan(30);
    damaged[at] ^= 0xff;
    await expect(readZip(damaged)).rejects.toThrow(/damaged/);
    await expect(readZip(new Uint8Array(MAX_ZIP_BYTES + 1))).rejects.toThrow(/larger than 5 MB/);
  });

  it("refuses more than 200 files", async () => {
    const many = Array.from({ length: MAX_FILES + 1 }, (_, i) => ({ name: `a/${i}.md`, data: enc("x") }));
    await expect(readZip(writeZip(many))).rejects.toThrow(/more than 200 files/);
  });

  it("refuses an encrypted archive", async () => {
    const z = writeZip([{ name: "a/x.md", data: enc("x") }]);
    const v = new DataView(z.buffer);
    // the central directory's flags: set the encryption bit
    for (let i = 0; i < z.length - 4; i++) if (v.getUint32(i, true) === 0x02014b50) v.setUint16(i + 8, v.getUint16(i + 8, true) | 1, true);
    await expect(readZip(z)).rejects.toThrow(/encrypted/);
  });
});

describe("yaml", () => {
  const roundTrip = (v: YamlValue) => parseYaml(stringifyYaml(v));

  it("writes and reads strings, numbers, booleans, null, arrays and maps", () => {
    const doc: YamlValue = {
      format: 1,
      name: "Daneel",
      purpose: "Research",
      enabled: true,
      model: null,
      ratio: 1.5,
      cadence: { preset: "weekdays", time: "08:00" },
      tags: ["a", "b c", "d"],
      empty_list: [],
      empty_map: {},
      nested: [{ kind: "schedule", cadence: { cron: "0 8 * * 1-5" } }, { kind: "webhook" }, ["x", "y"]],
    };
    expect(roundTrip(doc)).toEqual(doc);
  });

  it("quotes what could be read as something else, and escapes", () => {
    const tricky: YamlValue = {
      a: "true",
      b: "null",
      c: "123",
      d: "1e5",
      e: "yes",
      f: "has: colon",
      g: "# not a comment",
      h: "- not a list",
      i: "line one\nline two",
      j: "quote \" and backslash \\ and tab \t",
      k: "  padded  ",
      l: "emoji ✓ and é",
      m: "",
      n: "*star",
      o: "&amp",
      p: "!tag",
      q: "|pipe",
      "key with spaces": "x",
      "0numeric": "x",
    };
    const out = stringifyYaml(tricky);
    expect(out).toContain('a: "true"');
    expect(out).toContain('c: "123"');
    expect(roundTrip(tricky)).toEqual(tricky);
  });

  it("writes plain text plainly", () => {
    expect(stringifyYaml({ name: "Release notes", handle: "daneel-1" })).toBe("name: Release notes\nhandle: daneel-1\n");
  });

  it("reads comments and blank lines", () => {
    expect(parseYaml("# the agent\nformat: 1\n\nname: Daneel\n")).toEqual({ format: 1, name: "Daneel" });
    // A comment after a value is refused (the writer never writes one): quote the text to keep a #.
    expect(() => parseYaml("name: Daneel # trailing\n")).toThrow(YamlError);
    expect(parseYaml('name: "Daneel # not a comment"\n')).toEqual({ name: "Daneel # not a comment" });
  });

  it("refuses anchors, aliases, tags, block scalars, merge keys, flow collections, tabs, documents and duplicates", () => {
    const bad: Record<string, string> = {
      anchor: "a: &x 1\n",
      alias: "a: *x\n",
      tag: "a: !!str 1\n",
      tag2: "a: !custom x\n",
      block: "a: |\n  text\n",
      folded: "a: >\n  text\n",
      merge: "<<: {}\n",
      flow: "a: [1, 2]\n",
      flowmap: "a: {b: 1}\n",
      single: "a: 'x'\n",
      tab: "a:\n\tb: 1\n",
      document: "---\na: 1\n",
      duplicate: "a: 1\na: 2\n",
      indent: "a: 1\n  b: 2\n",
      bare_colon: "a: b: c\n",
    };
    for (const [name, src] of Object.entries(bad)) expect(() => parseYaml(src), name).toThrow(YamlError);
  });

  it("does not let a key named __proto__ reach the prototype", () => {
    const v = parseYaml('"__proto__":\n  polluted: true\n') as Record<string, unknown>;
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(Object.keys(v)).toEqual(["__proto__"]);
  });

  it("limits nesting and size", () => {
    let deep: YamlValue = "x";
    for (let i = 0; i < 40; i++) deep = { a: deep };
    expect(() => stringifyYaml(deep)).toThrow(YamlError);
    expect(() => parseYaml("a: 1\n".repeat(5001))).toThrow(YamlError);
  });

  it("reads a list under a key at the same column", () => {
    expect(parseYaml("tags:\n- a\n- b\nname: x\n")).toEqual({ tags: ["a", "b"], name: "x" });
  });
});
