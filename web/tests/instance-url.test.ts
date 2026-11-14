import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import { errorMessage, hostConflictMessage } from "@/lib/api/errors";
import { splitInstanceUrl } from "@/lib/instance-url";

describe("splitInstanceUrl (FIX-145)", () => {
  it("splits a full URL into host and path, dropping scheme, query and fragment", () => {
    expect(splitInstanceUrl("https://its.cern.ch/jira")).toEqual({ host: "its.cern.ch", path: "/jira" });
    expect(splitInstanceUrl("https://its.cern.ch/jira/")).toEqual({ host: "its.cern.ch", path: "/jira" });
    expect(splitInstanceUrl(" http://ITS.cern.ch/a/b?x=1#y ")).toEqual({ host: "its.cern.ch", path: "/a/b" });
    expect(splitInstanceUrl("https://gitlab.example.com")).toEqual({ host: "gitlab.example.com", path: "" });
    expect(splitInstanceUrl("https://gitlab.example.com/")).toEqual({ host: "gitlab.example.com", path: "" });
    expect(splitInstanceUrl("its.cern.ch/jira")).toEqual({ host: "its.cern.ch", path: "/jira" });
  });

  it("leaves a bare host, and anything with a port or userinfo, as typed", () => {
    expect(splitInstanceUrl("its.cern.ch")).toBeNull();
    expect(splitInstanceUrl("")).toBeNull();
    expect(splitInstanceUrl("https://its.cern.ch:8443/jira")).toBeNull();
    expect(splitInstanceUrl("its.cern.ch:8443")).toBeNull();
    expect(splitInstanceUrl("https://user@its.cern.ch/jira")).toBeNull();
  });
});

describe("instance messages (FIX-145)", () => {
  it("the holder of an overlapping path is named by its address", () => {
    const m = hostConflictMessage({ kind: "instance", scope: "org", app_name: "Jira", base_url: "https://its.cern.ch/jira" }, true);
    expect(m).toContain("overlaps");
    expect(m).toContain("(https://its.cern.ch/jira)");
    expect(m).toContain("in this organization");
  });

  it("an invalid path has a static message", () => {
    expect(errorMessage(new ApiError("instance_base_path_invalid", "x"))).toMatch(/must start with \//);
  });
});
