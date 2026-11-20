import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import { probeFailedMessage, probeFailure } from "@/lib/api/errors";

const H = "its.cern.ch";
const base = { url: "https://its.cern.ch/rest/api/2/serverInfo", expected: [200, 401] };

describe("probeFailedMessage (FIX-144)", () => {
  it("a 404 names the host, the path and the not-yet-supported path hint", () => {
    const m = probeFailedMessage({ ...base, reason: "unexpected_status", status: 404 }, "Jira Data Center", H);
    expect(m).toContain("its.cern.ch answered 404 Not Found at /rest/api/2/serverInfo.");
    expect(m).toContain("Jira Data Center answers there when it is served at the root of the host.");
    // FIX-145: the way out is the Path field
    expect(m).toContain("If Jira Data Center is served under a path, such as https://its.cern.ch/jira, enter it in Path.");
  });

  it("a 404 with a path entered says to check the path", () => {
    const m = probeFailedMessage({ ...base, url: "https://its.cern.ch/jira/rest/api/2/serverInfo", reason: "unexpected_status", status: 404 }, "Jira", H, "/jira");
    expect(m).toContain("answered 404 Not Found at /jira/rest/api/2/serverInfo.");
    expect(m).toContain("served under /jira on its.cern.ch. Check the Path.");
    expect(m).not.toContain("enter it in Path");
  });

  it("a redirect names the location; another host says to register that host", () => {
    const other = probeFailedMessage(
      { ...base, reason: "unexpected_status", status: 302, location: "https://sso.cern.ch/login" },
      "Jira",
      H,
    );
    expect(other).toContain("redirected /rest/api/2/serverInfo to https://sso.cern.ch/login.");
    expect(other).toContain("Register that host instead.");
    const same = probeFailedMessage(
      { ...base, reason: "unexpected_status", status: 301, location: "https://its.cern.ch/jira/rest/api/2/serverInfo" },
      "Jira",
      H,
    );
    expect(same).toContain("to https://its.cern.ch/jira/rest/api/2/serverInfo.");
    expect(same).toContain("enter it in Path.");
  });

  it("another status says what the app answers", () => {
    const m = probeFailedMessage({ ...base, reason: "unexpected_status", status: 503 }, "Jira", H);
    expect(m).toBe(
      "its.cern.ch answered 503 Service Unavailable at /rest/api/2/serverInfo; Jira answers 200 or 401 there. Check that this host runs Jira.",
    );
  });

  it.each([
    ["destination_refused", /can't reach .* private address\. Ask whoever installed Aurora to allow it\./],
    ["dns", /could not be found.*Check the spelling/],
    ["timeout", /did not answer within 10 seconds/],
    ["connect", /refused the connection on port 443/],
    ["tls", /certificate is not trusted.*extra CA bundle/],
  ])("reason %s", (reason, re) => {
    expect(probeFailedMessage({ ...base, reason }, "Jira", H)).toMatch(re);
  });

  it("unreachable and unknown reasons use the fallback", () => {
    for (const reason of ["unreachable", "something_new", undefined]) {
      expect(probeFailedMessage({ ...base, reason }, "Jira", H)).toBe("its.cern.ch could not be reached from the Aurora server.");
    }
  });

  it("reads the details of an instance_probe_failed error only", () => {
    const e = new ApiError("instance_probe_failed", "x", { reason: "dns", status: null });
    expect(probeFailure(e)?.reason).toBe("dns");
    expect(probeFailure(new ApiError("other", "x"))).toBeNull();
  });
});
