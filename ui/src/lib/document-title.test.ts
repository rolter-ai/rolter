import { describe, expect, test } from "bun:test";

import { documentTitle } from "./document-title";

describe("documentTitle", () => {
  test("puts the screen first and the lowercase name after it", () => {
    expect(documentTitle("Virtual keys")).toBe("Virtual keys · rolter");
    expect(documentTitle("Виртуальные ключи")).toBe("Виртуальные ключи · rolter");
  });

  test("falls back to the bare name when there is no title yet", () => {
    expect(documentTitle("")).toBe("rolter");
  });
});
