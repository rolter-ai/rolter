import { describe, expect, test } from "bun:test";

import { avatarColor } from "./avatar";

const PALETTE = [1, 2, 3, 4, 5, 6].map((n) => `var(--avatar-${n})`);

describe("avatarColor", () => {
  test("gives a person the same colour every time", () => {
    const id = "0d5b2b7e-6f0e-4d0c-9c3a-1a2b3c4d5e6f";
    expect(avatarColor(id)).toBe(avatarColor(id));
  });

  test("is one of the six palette tokens, never a raw colour", () => {
    for (const id of ["", "user-1", "ü", "a".repeat(500)]) {
      expect(PALETTE).toContain(avatarColor(id));
    }
  });

  // pinned: the colour is derived from the id, so a change to the hash
  // recolours every person on every deployment
  test("keeps the colours it has already handed out", () => {
    expect(avatarColor("user-1")).toBe("var(--avatar-5)");
    expect(avatarColor("user-2")).toBe("var(--avatar-2)");
    expect(avatarColor("user-3")).toBe("var(--avatar-3)");
  });

  test("does not depend on what else is in the list", () => {
    const ids = ["user-1", "user-2", "user-3", "user-4"];
    const all = ids.map(avatarColor);
    // a search leaves the last two rows; they keep the colours they had
    expect(ids.slice(2).map(avatarColor)).toEqual(all.slice(2));
    expect([...ids].reverse().map(avatarColor)).toEqual([...all].reverse());
  });

  test("spreads uuid-shaped ids over the whole palette", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) {
      seen.add(avatarColor(`${i.toString(16).padStart(8, "0")}-6f0e-4d0c-9c3a-1a2b3c4d5e6f`));
    }
    expect([...seen].sort()).toEqual(PALETTE);
  });
});
