import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { ScopeLadder, type LadderAbove, type LadderBelow } from "@/components/ui/scope-ladder";
import { wrap } from "./support/render";

afterEach(cleanup);

const admin = { action: "change_this", role: "instance_admin" as const, people: [], you_can: false };
const orgAdmins = { action: "change_this", role: "org_admin" as const, people: [{ name: "Ada Lovelace" }, { name: "Alan Turing" }], you_can: false };

const above: LadderAbove[] = [
  { label: "Requests without an account", value_display: "Blocked unless the allow-list names the website.", source: "instance", who_can: admin },
  { label: "Allow-list", value_display: "4 websites.", source: "org", who_can: orgAdmins },
];

const below: LadderBelow = {
  columns: ["Without an account", "Browser connections", "Rules"],
  rows: [
    { label: "Release engineering", values: ["Blocked", "Allow-list only", "3"], link: { href: "/o/acme/w/release/internet-access", label: "Open" } },
    { label: "Bob's workspace", values: ["Inherit", "Off", "0"], link: { href: "/o/acme/w/bob/internet-access", label: "Open" } },
  ],
};

describe("ScopeLadder (C-963)", () => {
  it("shows the limits from above with their source and who can change them", () => {
    render(<ScopeLadder above={above} here={<p>Workspace controls</p>} />);
    const list = screen.getByRole("list", { name: "Limits set above this level" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]!).getByText("Set for you by an Aurora administrator")).toBeInTheDocument();
    expect(within(rows[0]!).getByText(/An Aurora administrator can set this up\./)).toBeInTheDocument();
    expect(within(rows[1]!).getByText("Set for you by your organization's admins")).toBeInTheDocument();
    expect(within(rows[1]!).getByText(/Ask an organization admin\./)).toBeInTheDocument();
    expect(screen.getByText("Workspace controls")).toBeInTheDocument();
  });

  it("names the limit in the controls' own section when there is a minimum", () => {
    render(<ScopeLadder above={above} here={<p>Controls</p>} minimumNote="Looser options are not available: Administration requires blocking requests without an account." />);
    expect(screen.getByText(/Looser options are not available: Administration requires/)).toBeInTheDocument();
  });

  it("collapsed: a plain member at Use gets no Restrict further… and the controls stay out (D-406)", () => {
    wrap(<ScopeLadder collapsed above={above} here={<p>Four workspace controls</p>} />, { moreControls: false });
    expect(screen.queryByRole("button", { name: "Restrict further…" })).toBeNull();
    expect(screen.queryByText("Four workspace controls")).toBeNull();
  });

  it("collapsed with canTighten={false} draws nothing: no limits from above, no controls (D-444)", () => {
    const { container } = wrap(<ScopeLadder collapsed canTighten={false} above={above} here={<p>Four workspace controls</p>} />, { moreControls: true });
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByText("Requests without an account")).toBeNull();
    expect(screen.queryByRole("button", { name: "Restrict further…" })).toBeNull();
  });

  it("canTighten={false} does not change a ladder that is not collapsed", () => {
    render(<ScopeLadder canTighten={false} above={above} here={<p>Organization controls</p>} />);
    expect(screen.getByText("Requests without an account")).toBeInTheDocument();
    expect(screen.getByText("Organization controls")).toBeInTheDocument();
  });

  it("collapsed: the controls stay out of the page until Restrict further… is opened", () => {
    wrap(<ScopeLadder collapsed above={above} here={<p>Four workspace controls</p>} />, { moreControls: true });
    expect(screen.queryByText("Four workspace controls")).toBeNull();
    const button = screen.getByRole("button", { name: "Restrict further…" });
    expect(button.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(button);
    expect(screen.getByText("Four workspace controls")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hide" }).getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("Four workspace controls")).toBeNull();
  });

  it("not collapsed: the controls are open and there is no disclosure", () => {
    render(<ScopeLadder above={[]} here={<p>Organization controls</p>} />);
    expect(screen.getByText("Organization controls")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Restrict further/ })).toBeNull();
    expect(screen.queryByText(/^Set for you by/)).toBeNull();
  });

  it("below: a table of what the levels below restrict further, with a link per row, and a stacked copy for a phone", () => {
    render(<ScopeLadder above={[]} here={<p>Controls</p>} below={below} />);
    const table = screen.getByRole("table", { name: "What the levels below restrict further" });
    expect(within(table).getAllByRole("columnheader", { hidden: true }).map((h) => h.textContent)).toEqual(["", "Without an account", "Browser connections", "Rules", ""]);
    const row = within(table).getByText("Release engineering").closest("tr")!;
    expect(within(row).getByText("Allow-list only")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "Open" }).getAttribute("href")).toBe("/o/acme/w/release/internet-access");
    // The stacked list carries the same rows for 390 px.
    const stacked = screen.getByRole("list", { name: "What the levels below restrict further" });
    expect(within(stacked).getAllByRole("listitem")).toHaveLength(2);
    expect(within(stacked).getAllByText("Browser connections")).toHaveLength(2);
  });

  it("below, with no row, says nothing below restricts it further", () => {
    render(<ScopeLadder above={[]} here={<p>Controls</p>} below={{ columns: ["Rules"], rows: [], empty: "No workspace restricts this further." }} />);
    expect(screen.getByText("No workspace restricts this further.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });
});
