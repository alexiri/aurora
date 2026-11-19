import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Select } from "@/components/ui/select";

describe("Select", () => {
  it("lists the placeholder first, then the options, and reports the chosen value", () => {
    const onChange = vi.fn();
    render(
      <Select aria-label="Role" value="" placeholder="Pick one…" onChange={onChange} options={[{ value: "a", label: "Alpha" }, { value: "b", label: "Beta" }]} />,
    );
    const box = screen.getByLabelText("Role") as HTMLSelectElement;
    expect([...box.options].map((o) => [o.value, o.textContent])).toEqual([["", "Pick one…"], ["a", "Alpha"], ["b", "Beta"]]);
    fireEvent.change(box, { target: { value: "b" } });
    expect(onChange).toHaveBeenCalledWith("b");
  });

  it("takes grouped rows as children, keeps the input look and merges className", () => {
    render(
      <Select aria-label="Kind" value="x" onChange={() => {}} className="w-36" disabled>
        <option value="x">X</option>
      </Select>,
    );
    const box = screen.getByLabelText("Kind") as HTMLSelectElement;
    expect(box.disabled).toBe(true);
    expect(box.className).toContain("w-36");
    expect(box.className).toContain("rounded-control");
    expect(box.options).toHaveLength(1);
  });

  it("renders no rows without options", () => {
    render(<Select aria-label="Empty" value="" onChange={() => {}} />);
    expect((screen.getByLabelText("Empty") as HTMLSelectElement).options).toHaveLength(0);
  });
});
