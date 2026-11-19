import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { groupGaps } from "@/components/admin/sign-in-gaps";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import type { FieldInfo } from "@/lib/api/settings";
import { bytesWords, defaultInWords, editUnit } from "@/lib/settings-words";

describe("P5-POLISH-11 pass 1 review fixes", () => {
  it("a dialog focuses its first field, not the Close button (P5-POLISH-1)", async () => {
    render(
      <Dialog open>
        <DialogContent title="Add a thing">
          <input aria-label="Name" />
        </DialogContent>
      </Dialog>,
    );
    expect(await screen.findByLabelText("Name")).toHaveFocus();
    expect(screen.getByRole("button", { name: "Close" })).not.toHaveFocus();
  });

  it("a dialog without a field focuses the dialog itself, not Close", async () => {
    render(
      <Dialog open>
        <DialogContent title="Nothing to type">
          <p>Just words.</p>
        </DialogContent>
      </Dialog>,
    );
    expect(await screen.findByRole("dialog")).toHaveFocus();
  });

  it("byte counts read in GB or MB (P5-POLISH-10)", () => {
    expect(bytesWords(5368709120)).toBe("5 GB");
    expect(bytesWords(268435456)).toBe("256 MB");
    expect(defaultInWords("5368709120 bytes")).toBe("5 GB");
    expect(defaultInWords("15 minutes")).toBe("15 minutes");
  });

  it("byte settings follow the server's default in words: \"5 GiB\" edits in GB, \"256 KiB\" in bytes (P5-POLISH-11 pass 2)", () => {
    // `aurora-config` fields.rs writes byte defaults as "5 GiB", "1 MiB", "256 KiB", never "<n> bytes".
    const field = (default_display: string) => ({ key: "k", label: "l", help: "", kind: "int", unit: "bytes", default_display, choices: null, doc: null }) as unknown as FieldInfo;
    expect(defaultInWords("5 GiB")).toBe("5 GB");
    expect(defaultInWords("1 MiB")).toBe("1 MB");
    expect(editUnit(field("5 GiB"))).toEqual({ size: 1024 ** 3, label: "GB" });
    expect(editUnit(field("1 MiB"))).toEqual({ size: 1024 ** 2, label: "MB" });
    expect(editUnit(field("256 KiB"))).toEqual({ size: 1, label: "bytes" });
  });

  it("sign-in gaps of one package are one group (P5-POLISH-10)", () => {
    const gap = (appId: string, pkg: string) => ({ appId, name: appId, pkg, tokenOnly: false });
    const groups = groupGaps([gap("calendar", "google-workspace"), gap("notion", "notion"), gap("gmail", "google-workspace")]);
    expect(groups.map((g) => g.map((x) => x.appId))).toEqual([["calendar", "gmail"], ["notion"]]);
  });
});
