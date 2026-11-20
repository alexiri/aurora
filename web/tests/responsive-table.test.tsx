import { readFileSync } from "node:fs";
import { join } from "node:path";

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ResponsiveTable } from "@/components/ui/responsive-table";

interface Person {
  id: string;
  name: string;
  email: string;
  role: string;
}
const rows: Person[] = [
  { id: "1", name: "Alice Admin", email: "alice@example.test", role: "Owner" },
  { id: "2", name: "Bob Builder", email: "bob@example.test", role: "Member" },
];

describe("ResponsiveTable", () => {
  it("renders one table (a single DOM) with the title cell first, labelled meta cells and the actions cell", () => {
    render(
      <ResponsiveTable
        caption="People"
        title={{ header: "Person", cell: (p) => p.name }}
        columns={[
          { key: "email", header: "E-mail", cell: (p) => p.email },
          { key: "role", header: "Role", cell: (p) => p.role, label: false },
        ]}
        rows={rows}
        rowKey={(p) => p.id}
        actions={(p) => <button>{`More for ${p.name}`}</button>}
      />,
    );
    const table = screen.getByRole("table", { name: "People" });
    expect(table).toHaveAttribute("data-stack", "900");
    expect(screen.getAllByRole("row")).toHaveLength(3);
    const first = screen.getAllByRole("row")[1]!;
    expect(first.querySelector("[data-cell=title]")).toHaveTextContent("Alice Admin");
    expect(first.querySelector("[data-cell=actions]")).toContainElement(within(first).getByRole("button", { name: "More for Alice Admin" }));
    // Labelled on the stacked row ("E-mail alice@…"); the role is a chip without its label.
    expect(first.querySelector("[data-label='E-mail']")).toHaveTextContent("alice@example.test");
    expect(first.querySelector("[data-label='Role']")).toBeNull();
    // Nobody is rendered twice: names appear once.
    expect(screen.getAllByText("Alice Admin")).toHaveLength(1);
  });

  it("has no inner scroll container below its breakpoint (the scroll is only from the breakpoint up)", () => {
    const { container } = render(
      <DataTable caption="Files" stackBelow={900}>
        <thead>
          <tr>
            <Th>Name</Th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <Td cell="title">a.txt</Td>
          </tr>
        </tbody>
      </DataTable>,
    );
    const region = container.querySelector("[role=region]")!;
    expect(region.className).toContain("min-[900px]:overflow-x-auto");
    expect(region.className).not.toMatch(/(^|\s)overflow-x-auto/);
    expect(container.querySelector("table")!.className).toContain("min-[900px]:min-w-max");
  });

  it("keeps today's scrolling table without stackBelow", () => {
    const { container } = render(
      <DataTable caption="Plain">
        <tbody>
          <tr>
            <Td>x</Td>
          </tr>
        </tbody>
      </DataTable>,
    );
    expect(container.querySelector("[role=region]")!.className).toMatch(/(^|\s)overflow-x-auto/);
    expect(container.querySelector("table")).not.toHaveAttribute("data-stack");
  });

  it("the stylesheet turns a data-stack table into cards below the breakpoint", () => {
    const css = readFileSync(join(__dirname, "..", "src", "app", "globals.css"), "utf8");
    expect(css).toMatch(/@media \(max-width: 899\.98px\)[\s\S]*table\[data-stack="900"\] > tbody > tr \{[^}]*display: grid/);
    expect(css).toMatch(/td\[data-cell="actions"\][^}]*justify-self: end/);
    expect(css).toMatch(/td\[data-label\]::before/);
  });
});
