import { describe, expect, it } from "bun:test";

import { decisionRequest, pickDecisionModel } from "./decisions";

const questions = JSON.stringify({
  visible: { type: "noul", instructions: "Is a parcel visible?" },
});

describe("decision requests", () => {
  it("preserves structured state and sends pixels in OpenJev image_data, not chat content", () => {
    const request = decisionRequest(
      "jev",
      '{"text":"An image: <<IMG>>"}',
      true,
      questions,
      "data:image/png;base64,eA==",
    );
    expect(request.state).toEqual({ text: "An image: <<IMG>>" });
    expect(request.image_data).toBe("data:image/png;base64,eA==");
    expect(request.questions.visible.type).toBe("noul");
    expect(request).not.toHaveProperty("messages");
    expect(request).not.toHaveProperty("images");
  });

  it("keeps plain text and arbitrary question ids and extensions", () => {
    const q = JSON.stringify({
      "risk/1": { type: "span", instructions: "Locate it", extension: true },
    });
    expect(decisionRequest("m", "hello", false, q)).toEqual({
      model: "m",
      state: "hello",
      questions: JSON.parse(q),
    });
  });

  it("rejects invalid JSON, null state, empty questions and malformed questions", () => {
    for (const q of ["{", "[]", "{}", '{"q":null}', '{"q":{"type":"noul"}}']) {
      expect(() => decisionRequest("m", "text", false, q)).toThrow();
    }
    for (const state of ["null", "true", "42", "{"]) {
      expect(() => decisionRequest("m", state, true, questions)).toThrow();
    }
  });
});

describe("pickDecisionModel", () => {
  const offered = [{ id: "openjev" }, { id: "fake-llm" }];

  it("keeps the model chosen while it is still on offer", () => {
    expect(pickDecisionModel(offered, "openjev", "fake-llm")).toBe("openjev");
  });

  it("falls back to the builtin, then to the first model on offer", () => {
    expect(pickDecisionModel(offered, "gone", "fake-llm")).toBe("fake-llm");
    // a route named fake-llm shadows the builtin and leaves the decision list
    expect(pickDecisionModel([{ id: "openjev" }], "fake-llm", "fake-llm")).toBe("openjev");
  });

  it("names nothing when no model serves decisions", () => {
    expect(pickDecisionModel([], "fake-llm", "fake-llm")).toBe("");
  });
});
