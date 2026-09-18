import { describe, expect, it } from "bun:test";
import * as React from "react";

import { SCREENS } from "@/App";

// #1709: the dashboard shipped as one chunk, so the first paint of the sign-in
// screen carried every screen in the rail. Splitting it is one `React.lazy` per
// entry in `SCREENS`, and the way that regresses is silent: a `import Foo from
// "@/pages/Foo"` added at the top of `App.tsx` for one new screen pulls that
// screen — and everything it imports — back into the entry chunk, and the build
// still succeeds. Nothing in the build output says which screen did it.
describe("every navigable screen is its own chunk", () => {
  const LAZY = Symbol.for("react.lazy");

  it("renders through React.lazy rather than a static import", () => {
    const eager = Object.entries(SCREENS)
      .filter(([, node]) => {
        const type = React.isValidElement(node) ? (node.type as { $$typeof?: symbol }) : undefined;
        return type?.$$typeof !== LAZY;
      })
      .map(([key]) => key);

    expect(eager).toEqual([]);
  });

  it("covers every screen, so the check cannot pass by finding nothing", () => {
    expect(Object.keys(SCREENS).length).toBeGreaterThan(40);
  });
});
