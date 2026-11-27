import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";

import { MarkdownEditor } from "@/components/ui/markdown-editor";
import { splitByTerms } from "@/components/ui/markdown-marks";

// (P5-MEMORY-2) The shared Write / Preview editor, and the search marks of the Markdown renderer.

function Harness({ start = "", error }: { start?: string; error?: string }) {
  const [value, setValue] = useState(start);
  return <MarkdownEditor id="body" label="Instructions" value={value} onChange={setValue} error={error} placeholder="Write here." />;
}

describe("MarkdownEditor", () => {
  it("is a labelled textarea with Write and Preview tabs and the hint", () => {
    render(<Harness />);
    const box = screen.getByRole("textbox", { name: "Instructions" });
    expect(box).toHaveAttribute("placeholder", "Write here.");
    expect(box).toHaveAccessibleDescription("Markdown works: # headings, - lists, - [ ] checklists, tables");
    expect(screen.getByRole("tab", { name: "Write", selected: true })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Preview", selected: false })).toBeInTheDocument();
    for (const tab of screen.getAllByRole("tab")) expect(document.getElementById(tab.getAttribute("aria-controls")!)).not.toBeNull();
  });

  it("renders the text in Preview and keeps it when going back; arrow keys move between the tabs", () => {
    render(<Harness />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "# Title\n\n- one" } });
    fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
    expect(screen.getByRole("tabpanel", { name: "Preview" })).toContainElement(screen.getByRole("heading", { name: "Title" }));
    expect(screen.queryByRole("textbox")).toBeNull();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Preview" }), { key: "ArrowLeft" });
    expect(screen.getByRole("textbox")).toHaveValue("# Title\n\n- one");
  });

  it("says when there is nothing to preview, and links an error to the field", () => {
    render(<Harness error="Write something." />);
    expect(screen.getByRole("textbox")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("Write something.");
    expect(screen.getByRole("textbox").getAttribute("aria-describedby")).toContain("body-error");
    fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
    expect(screen.getByText("Nothing to preview yet.")).toBeInTheDocument();
  });
});

describe("search marks", () => {
  it("splits text at every occurrence of the terms, case-insensitively, taking the longer one first", () => {
    expect(splitByTerms("Rotation and rotations", ["rotation", "rotations"])).toEqual([
      { text: "Rotation", mark: true },
      { text: " and ", mark: false },
      { text: "rotations", mark: true },
    ]);
    expect(splitByTerms("a.b (x)", ["(x)"])).toEqual([{ text: "a.b ", mark: false }, { text: "(x)", mark: true }]);
    expect(splitByTerms("plain", [" ", ""])).toEqual([{ text: "plain", mark: false }]);
  });
});
