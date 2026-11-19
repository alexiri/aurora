import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { FieldControl } from "@/components/apps/dynamic-form";
import { ValueInput } from "@/components/credentials/value-input";
import { Field } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";

// FIX-173 (D-375): one secret field with Show / Hide.

afterEach(cleanup);

function Harness({ initial = "", disabled = false, onSubmit }: { initial?: string; disabled?: boolean; onSubmit?: () => void }) {
  const [v, setV] = useState(initial);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit?.();
      }}
    >
      <SecretInput id="s" aria-label="Link" value={v} onChange={setV} disabled={disabled} />
      <button type="button" onClick={() => setV("")}>
        parent clear
      </button>
    </form>
  );
}

const input = () => document.getElementById("s") as HTMLInputElement;

describe("SecretInput", () => {
  it("starts hidden with the no-spellcheck attributes", () => {
    render(<Harness />);
    expect(input().type).toBe("password");
    expect(input().getAttribute("spellcheck")).toBe("false");
    expect(input().getAttribute("autocapitalize")).toBe("off");
    expect(input().getAttribute("autocorrect")).toBe("off");
    expect(input().autocomplete).toBe("off");
    const btn = screen.getByRole("button", { name: "Show Link" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    expect(btn).toHaveAttribute("aria-controls", "s");
    expect(btn).toHaveAttribute("type", "button");
  });

  it("Show reveals what was typed, Hide masks it, the value never changes", () => {
    render(<Harness />);
    fireEvent.change(input(), { target: { value: "https://cal.example/secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Show Link" }));
    expect(input().type).toBe("text");
    expect(input().value).toBe("https://cal.example/secret");
    const hide = screen.getByRole("button", { name: "Hide Link" });
    expect(hide).toHaveAttribute("aria-pressed", "true");
    expect(hide).toHaveTextContent("Hide");
    fireEvent.click(hide);
    expect(input().type).toBe("password");
    expect(input().value).toBe("https://cal.example/secret");
  });

  it("does not submit the form when the button is pressed", () => {
    const onSubmit = vi.fn();
    render(<Harness onSubmit={onSubmit} />);
    fireEvent.click(screen.getByRole("button", { name: "Show Link" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("hides again when the parent clears the value, but not when the person deletes the text", () => {
    render(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Show Link" }));
    fireEvent.change(input(), { target: { value: "abc" } });
    fireEvent.change(input(), { target: { value: "" } });
    expect(input().type).toBe("text");
    fireEvent.change(input(), { target: { value: "abc" } });
    fireEvent.click(screen.getByRole("button", { name: "parent clear" }));
    expect(input().value).toBe("");
    expect(input().type).toBe("password");
  });

  it("hides again when it becomes disabled, and hidden again on unmount (a fresh mount starts hidden)", () => {
    const { rerender, unmount } = render(<Harness initial="x" />);
    fireEvent.click(screen.getByRole("button", { name: "Show Link" }));
    expect(input().type).toBe("text");
    rerender(<Harness initial="x" disabled />);
    expect(input().type).toBe("password");
    unmount();
    render(<Harness initial="x" />);
    expect(input().type).toBe("password");
  });

  it("falls back to plain Show / Hide without a label", () => {
    render(<SecretInput id="plain" value="" onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "Show" })).toBeInTheDocument();
  });

  it("gives two fields in one form distinct buttons", () => {
    render(
      <>
        <SecretInput id="a" revealLabel="client secret" value="" onChange={() => {}} />
        <SecretInput id="b" revealLabel="signing secret" value="" onChange={() => {}} />
      </>,
    );
    expect(screen.getByRole("button", { name: "Show client secret" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show signing secret" })).toBeInTheDocument();
  });

  it("links Field's hint and error to the input", () => {
    render(
      <Field id="f" label="Token" hint="A hint." error="Too short">
        <SecretInput id="f" value="" onChange={() => {}} />
      </Field>,
    );
    const el = document.getElementById("f") as HTMLInputElement;
    expect(el.getAttribute("aria-invalid")).toBe("true");
    expect(el.getAttribute("aria-describedby")).toBe("f-error");
  });
});

describe("ValueInput", () => {
  it("links the hint-less error to the input (aria-describedby, aria-invalid)", () => {
    render(<ValueInput id="v" label="Value" value="" onChange={() => {}} error="Required" />);
    const el = screen.getByLabelText("Value", { selector: "input" });
    expect(el.getAttribute("aria-invalid")).toBe("true");
    expect(el.getAttribute("aria-describedby")).toBe("v-error");
    expect((el as HTMLInputElement).type).toBe("password");
    fireEvent.click(screen.getByRole("button", { name: "Show value" }));
    expect((el as HTMLInputElement).type).toBe("text");
  });
});

describe("FieldControl secret branch (ICS link, CalDAV app password)", () => {
  function Form({ label }: { label: string }) {
    const [v, setV] = useState("");
    return (
      <FieldControl
        f={{ name: "url", label, type: "secret", required: true } as never}
        value={v}
        onChange={setV}
      />
    );
  }

  it.each(["Secret ICS link", "CalDAV app password"])("reveals the typed %s and hides it again", (label) => {
    render(<Form label={label} />);
    const el = document.getElementById("cf-url") as HTMLInputElement;
    expect(el.type).toBe("password");
    fireEvent.change(el, { target: { value: "typed-value" } });
    fireEvent.click(screen.getByRole("button", { name: `Show ${label}` }));
    expect(el.type).toBe("text");
    expect(el.value).toBe("typed-value");
    fireEvent.click(screen.getByRole("button", { name: `Hide ${label}` }));
    expect(el.type).toBe("password");
    expect(el.value).toBe("typed-value");
  });
});
