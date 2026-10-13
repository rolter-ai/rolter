import { describe, expect, test } from "bun:test";

import { csvCell, csvLine } from "./csv";

describe("csvCell", () => {
  test("leaves a plain value alone", () => {
    expect(csvCell("prod")).toBe("prod");
    expect(csvCell(null)).toBe("");
    expect(csvCell(false)).toBe("false");
  });

  test("quotes a comma, a quote and a line break", () => {
    expect(csvCell("billing, eu")).toBe('"billing, eu"');
    expect(csvCell('say "hi"')).toBe('"say ""hi"""');
    expect(csvCell("a\nb")).toBe('"a\nb"');
  });

  test("defuses a leading formula character", () => {
    for (const lead of ["=", "+", "-", "@"]) expect(csvCell(`${lead}1+1`)).toBe(`'${lead}1+1`);
  });

  test("a line keeps one column per value", () => {
    expect(csvLine(["billing, eu", "=SUM(A1)", 1])).toBe(`"billing, eu",'=SUM(A1),1`);
  });
});
