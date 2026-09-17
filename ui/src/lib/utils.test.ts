import { describe, expect, it } from "bun:test";

import { cn } from "@/lib/utils";

describe("cn", () => {
  it("joins class names", () => {
    expect(cn("rounded", "border")).toBe("rounded border");
  });

  it("drops falsy values, so a conditional class can be inlined", () => {
    expect(cn("rounded", false && "hidden", undefined, null, "border")).toBe("rounded border");
  });

  it("lets the last of two conflicting tailwind utilities win", () => {
    // the reason this helper exists: a caller's `className` has to be able to
    // override a component's default, which plain concatenation cannot do
    expect(cn("px-2", "px-4")).toBe("px-4");
    expect(cn("text-status-danger-text", "text-muted-foreground")).toBe("text-muted-foreground");
  });

  it("keeps utilities that only look alike", () => {
    expect(cn("px-2", "py-4")).toBe("px-2 py-4");
  });

  it("resolves conflicts across responsive and state variants separately", () => {
    expect(cn("p-2", "md:p-4")).toBe("p-2 md:p-4");
    expect(cn("hover:bg-muted", "hover:bg-accent")).toBe("hover:bg-accent");
  });

  it("flattens arrays and objects the way callers pass them", () => {
    expect(cn(["rounded", "border"], { hidden: false, flex: true })).toBe("rounded border flex");
  });

  it("is empty for no input", () => {
    expect(cn()).toBe("");
  });
});
