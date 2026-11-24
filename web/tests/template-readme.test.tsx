import { cleanup, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { TemplateReadme } from "@/components/templates/template-readme";
import { wrap } from "./support/render";

// C-1116 (acceptance: the README rendering rules): a hostile README — raw HTML, a `javascript:`
// link, a remote image and a relative link — renders none of them active. Headings shift under the
// page's own heading; https links open in a new tab, marked as external.

const HOSTILE = [
  "# Schedule manager",
  "",
  "<img src=x onerror=alert(1)>",
  "",
  "[click](javascript:alert(1))",
  "",
  "![remote picture](https://evil.test/picture.png)",
  "",
  "[docs](./other.md)",
  "",
  "[fine](https://example.test/guide)",
  "",
  "question@example.test",
].join("\n");

describe("TemplateReadme (C-1116)", () => {
  afterEach(cleanup);

  it("renders a hostile README with nothing active", () => {
    wrap(<TemplateReadme source={HOSTILE} />);
    // the markup is text, never an element
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeTruthy();
    expect(document.querySelectorAll("img")).toHaveLength(0);
    // the remote image is its alt text
    expect(screen.getByText("remote picture")).toBeTruthy();
    // javascript: is not a link; the label is text
    expect(document.querySelector("a[href^='javascript:']")).toBeNull();
    expect(screen.getByText("click")).toBeTruthy();
    // a relative link is plain text
    expect(document.querySelector("a[href='./other.md']")).toBeNull();
    expect(screen.getByText("docs")).toBeTruthy();
    // https stays a link, in a new tab, marked external
    const a = screen.getByText("fine").closest("a")!;
    expect(a.getAttribute("href")).toBe("https://example.test/guide");
    expect(a.getAttribute("target")).toBe("_blank");
    expect(a.getAttribute("rel")).toContain("noopener");
    expect(a.textContent).toContain("external link");
    // the address is text, not a mailto:... wait, mailto IS allowed; it stays a link
    const mail = document.querySelector("a[href^='mailto:']");
    expect(mail?.getAttribute("target")).toBe("_blank");
  });

  it("shifts headings under the page's own heading", () => {
    wrap(<TemplateReadme source={"# Top\n\n## Section\n\n### Sub"} />);
    expect(document.querySelector("h1")).toBeNull();
    expect(document.querySelector("h2")).toBeNull();
    expect(screen.getByRole("heading", { level: 3, name: "Top" })).toBeTruthy();
    expect(screen.getByRole("heading", { level: 4, name: "Section" })).toBeTruthy();
    expect(screen.getByRole("heading", { level: 5, name: "Sub" })).toBeTruthy();
  });

  it("renders ordinary markdown: lists, emphasis, code", () => {
    wrap(<TemplateReadme source={"- one\n- two\n\n**bold** and `code`"} />);
    expect(screen.getByRole("list").children).toHaveLength(2);
    expect(screen.getByText("bold")).toBeTruthy();
    expect(screen.getByText("code")).toBeTruthy();
  });
});
