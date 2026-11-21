import { describe, expect, it } from "vitest";

import { LINK_DIRECTION_CHOICES, OTHER_AGENT, directionSentence, directionTitle } from "@/lib/api/peers";

// P5-REFINE-10 (C-1085, channels-1): the "Link another agent" dialog's three choices, as the mockup words them.
describe("link direction words", () => {
  it("says the mockup's sentences for the three choices, from Daneel's settings", () => {
    expect(LINK_DIRECTION_CHOICES).toEqual(["both", "to_peer", "from_peer"]);
    expect(LINK_DIRECTION_CHOICES.map((d) => directionTitle("Daneel", "Rex", d))).toEqual([
      "Both ways",
      "Daneel can hand tasks to Rex",
      "Rex can hand tasks to Daneel",
    ]);
    expect(LINK_DIRECTION_CHOICES.map((d) => directionSentence("Daneel", "Rex", d))).toEqual([
      "Daneel and Rex can each give the other tasks.",
      "Rex only works on what Daneel sends.",
      "Daneel only works on what Rex sends.",
    ]);
  });

  it("with no agent chosen yet, a sentence or title that starts with 'the other agent' is capitalised", () => {
    expect(directionSentence("Daneel", OTHER_AGENT, "outgoing")).toBe("The other agent only works on what Daneel sends.");
    expect(directionTitle("Daneel", OTHER_AGENT, "incoming")).toBe("The other agent can hand tasks to Daneel");
    expect(directionTitle("Daneel", OTHER_AGENT, "outgoing")).toBe("Daneel can hand tasks to the other agent");
    expect(directionSentence("Daneel", OTHER_AGENT, "both")).toBe("Daneel and the other agent can each give the other tasks.");
  });

  it("reads a stored link's direction the same way (outgoing = to_peer, incoming = from_peer)", () => {
    expect(directionSentence("Daneel", "Rex", "both")).toBe("Daneel and Rex can each give the other tasks.");
    expect(directionSentence("Daneel", "Rex", "outgoing")).toBe("Rex only works on what Daneel sends.");
    expect(directionSentence("Daneel", "Rex", "incoming")).toBe("Daneel only works on what Rex sends.");
    expect(directionTitle("Rex", "Daneel", "incoming")).toBe("Daneel can hand tasks to Rex");
  });
});
