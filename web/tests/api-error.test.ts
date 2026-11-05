import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";

describe("ApiError.fromBody", () => {
  it("parses the error envelope", () => {
    const e = ApiError.fromBody({ error: { code: "not_found", message: "nope", details: { id: 1 } } }, 404);
    expect(e).toBeInstanceOf(ApiError);
    expect(e.code).toBe("not_found");
    expect(e.message).toBe("nope");
    expect(e.details).toEqual({ id: 1 });
    expect(e.status).toBe(404);
  });

  it("falls back on malformed bodies", () => {
    expect(ApiError.fromBody("boom").code).toBe("unknown");
  });
});
