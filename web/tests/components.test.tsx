import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentsList } from "@/components/agents-list";
import { CreateWorkspaceForm } from "@/components/create-workspace-form";
import { ApiError } from "@/lib/api/client";
import type { Agent } from "@/lib/api/queries";

afterEach(cleanup);

const agent = (o: Partial<Agent>): Agent =>
  ({ id: "1", handle: "daneel", name: "Daneel", kind: "hosted", is_default: true, ...o }) as Agent;

describe("AgentsList", () => {
  it("renders name, handle, kind and default badge", () => {
    render(<AgentsList agents={[agent({}), agent({ id: "2", name: "Other", handle: "o", kind: "byo", is_default: false })]} />);
    expect(screen.getByText("Daneel")).toBeInTheDocument();
    expect(screen.getByText("@daneel")).toBeInTheDocument();
    expect(screen.getByText("byo")).toBeInTheDocument();
    expect(screen.getAllByText("Default")).toHaveLength(1);
  });
  it("shows empty state", () => {
    render(<AgentsList agents={[]} />);
    expect(screen.getByText("No agents yet")).toBeInTheDocument();
  });
});

describe("CreateWorkspaceForm", () => {
  it("blocks submit on invalid input", () => {
    const onSubmit = vi.fn();
    render(<CreateWorkspaceForm onSubmit={onSubmit} pending={false} error={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    expect(screen.getByText("Name is required.")).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });
  it("submits valid values", () => {
    const onSubmit = vi.fn();
    render(<CreateWorkspaceForm onSubmit={onSubmit} pending={false} error={null} />);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Team" } });
    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ name: "Team", slug: null, memory_language: "english" }));
  });
  it("shows server field and general errors", () => {
    const f = new ApiError("invalid_request", "bad", { fields: [{ path: "timezone", message: "unknown zone" }] }, 400);
    const { rerender } = render(<CreateWorkspaceForm onSubmit={vi.fn()} pending={false} error={f} />);
    expect(screen.getByText("unknown zone")).toBeInTheDocument();
    rerender(<CreateWorkspaceForm onSubmit={vi.fn()} pending={false} error={new ApiError("slug_taken", "x", null, 409)} />);
    expect(screen.getByRole("alert")).toHaveTextContent(/already in use/);
  });
});
