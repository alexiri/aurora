import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { landingPath, pickDefaultWorkspace } from "@/lib/nav/landing";
import { validateNameSlug } from "@/lib/nav/validation";
import { loginUrl } from "@/lib/auth/use-unauthorized-redirect";

describe("landingPath", () => {
  it("returns null with no orgs", () => {
    expect(landingPath([], undefined)).toBeNull();
  });
  it("prefers the default workspace", () => {
    expect(landingPath([{ slug: "acme" }], [{ slug: "a" }, { slug: "default" }])).toBe(
      "/o/acme/w/default",
    );
  });
  it("falls back to first workspace, then org page", () => {
    expect(landingPath([{ slug: "acme" }, { slug: "b" }], [{ slug: "x" }])).toBe("/o/acme/w/x");
    expect(landingPath([{ slug: "acme" }], [])).toBe("/o/acme");
    expect(pickDefaultWorkspace([])).toBeUndefined();
  });
});

describe("errors", () => {
  it("extracts field errors and strips leading slash", () => {
    const e = new ApiError("invalid_request", "bad", { fields: [{ path: "/slug", message: "no" }] }, 400);
    expect(fieldErrors(e)).toEqual({ slug: "no" });
    expect(fieldErrors(new Error("x"))).toEqual({});
  });
  it("maps codes", () => {
    expect(errorMessage(new ApiError("org_creation_forbidden", "m", null, 403))).toMatch(/administrator/);
  });
});

describe("misc", () => {
  it("validates name and slug", () => {
    expect(validateNameSlug({ name: " ", slug: "" }).name).toBeTruthy();
    expect(validateNameSlug({ name: "ok", slug: "Bad Slug" }).slug).toBeTruthy();
    expect(validateNameSlug({ name: "ok", slug: "good-1" })).toEqual({});
  });
  it("builds safe login urls", () => {
    expect(loginUrl("/o/a?x=1")).toBe("/auth/login?next=%2Fo%2Fa%3Fx%3D1");
    expect(loginUrl("//evil.com")).toBe("/auth/login?next=%2F");
  });
});
