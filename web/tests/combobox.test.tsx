import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Combobox } from "@/components/ui/combobox";

// (P5-REFINE-14 send-back) The searchable select: type to narrow, keyboard to choose, Escape keeps the choice.

afterEach(cleanup);

const OPTIONS = [
  { value: "Europe/Zurich", label: "Europe/Zurich · Acme workspace" },
  { value: "Europe/London", label: "Europe/London" },
  { value: "Asia/Tokyo", label: "Asia/Tokyo" },
  { value: "America/New_York", label: "America/New_York" },
];

function Harness({ onChange }: { onChange?: (v: string) => void }) {
  const [v, setV] = useState("Europe/Zurich");
  return (
    <Combobox
      aria-label="Time zone"
      options={OPTIONS}
      value={v}
      onChange={(x) => {
        setV(x);
        onChange?.(x);
      }}
    />
  );
}

describe("Combobox", () => {
  it("shows the chosen label closed, and the whole list when focused", () => {
    render(<Harness />);
    const box = screen.getByRole("combobox", { name: "Time zone" });
    expect(box).toHaveValue("Europe/Zurich · Acme workspace");
    expect(box).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.focus(box);
    expect(box).toHaveAttribute("aria-expanded", "true");
    expect(within(screen.getByRole("listbox")).getAllByRole("option")).toHaveLength(4);
    // The chosen one is active first.
    expect(box).toHaveAttribute("aria-activedescendant", screen.getAllByRole("option")[0]!.id);
  });

  it("narrows as you type (label or value, any case) and says when nothing matches", () => {
    render(<Harness />);
    const box = screen.getByRole("combobox", { name: "Time zone" });
    fireEvent.focus(box);
    fireEvent.change(box, { target: { value: "EUROPE" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Europe/Zurich · Acme workspace", "Europe/London"]);
    fireEvent.change(box, { target: { value: "acme" } });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    fireEvent.change(box, { target: { value: "zzz" } });
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.getByText("No match")).toBeInTheDocument();
  });

  it("chooses with Down and Enter, wraps at the ends, and Escape keeps the old choice", () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "Time zone" });
    fireEvent.focus(box);
    fireEvent.keyDown(box, { key: "ArrowUp" }); // from the first row it wraps to the last
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith("America/New_York");
    expect(box).toHaveValue("America/New_York");
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.focus(box);
    fireEvent.change(box, { target: { value: "tok" } });
    fireEvent.keyDown(box, { key: "Escape" });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(box).toHaveValue("America/New_York");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("chooses with the pointer without losing focus first, and leaving closes the list", () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const box = screen.getByRole("combobox", { name: "Time zone" });
    fireEvent.focus(box);
    fireEvent.mouseDown(screen.getByRole("option", { name: "Asia/Tokyo" }));
    expect(onChange).toHaveBeenCalledWith("Asia/Tokyo");
    expect(box).toHaveValue("Asia/Tokyo");
    fireEvent.focus(box);
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.blur(box);
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
