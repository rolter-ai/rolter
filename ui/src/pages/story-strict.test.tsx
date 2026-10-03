import { afterAll, beforeAll, beforeEach, describe, expect, it } from "bun:test";
import * as React from "react";
import { createRoot, type Root } from "react-dom/client";

import {
  doubleInvokeFailure,
  resetStrictProbe,
  StrictLater,
  StrictProbe,
  strictProbeCounts,
} from "./story-strict";

// #1887: `expectDoubleInvoked` is only worth having if it fails when StrictMode
// doubled nothing. These run the real react-dom reconciler against the two
// shapes a story can take and check the probe and the verdict on each — the
// same-commit shape has to read one mount and no cleanup and be refused.
//
// There is no DOM under `bun test`, and none is needed: every component here
// renders `null` or a fragment, so react-dom never creates a host node. It
// only touches the container, its document and `window` on the way in, and
// the stubs below are that much and no more.

const container = {
  nodeType: 1,
  nodeName: "DIV",
  tagName: "DIV",
  namespaceURI: "http://www.w3.org/1999/xhtml",
  addEventListener() {},
  removeEventListener() {},
  ownerDocument: { addEventListener() {}, removeEventListener() {} },
} as unknown as HTMLElement;

const globals = globalThis as { window?: unknown; IS_REACT_ACT_ENVIRONMENT?: boolean };

beforeAll(() => {
  globals.window = { HTMLIFrameElement: class {}, document: { activeElement: null } };
  globals.IS_REACT_ACT_ENVIRONMENT = true;
});

afterAll(() => {
  delete globals.window;
  delete globals.IS_REACT_ACT_ENVIRONMENT;
});

beforeEach(() => resetStrictProbe());

/**
 * Storybook's own wrapper: `renderToCanvas` mounts every story as
 * `<ErrorBoundary key={storyId}><Story /></ErrorBoundary>`, so the fiber React
 * places is a component above the story's `StrictMode`, never the StrictMode.
 */
function Boundary({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}

/** A root that has already committed once, as Storybook's has by the time a story mounts. */
async function liveRoot(): Promise<Root> {
  const root = createRoot(container);
  await React.act(async () => root.render(null));
  return root;
}

async function story(root: Root, body: React.ReactNode): Promise<void> {
  await React.act(async () => root.render(<Boundary key="story">{body}</Boundary>));
}

describe("the StrictMode probe", () => {
  it("reads one mount and no cleanup in the same-commit shape, and that is refused", async () => {
    const root = await liveRoot();
    await story(
      root,
      <React.StrictMode>
        <StrictProbe />
      </React.StrictMode>,
    );
    expect(strictProbeCounts()).toEqual({ mounts: 1, cleanups: 0 });
    expect(doubleInvokeFailure(strictProbeCounts())).toContain("did not double-invoke");
    await React.act(async () => root.unmount());
  });

  it("reads two mounts and one cleanup when the subject lands in a later commit", async () => {
    const root = await liveRoot();
    const host = (mounted: boolean) => (
      <React.StrictMode>
        <StrictLater mounted={mounted}>{null}</StrictLater>
      </React.StrictMode>
    );
    await story(root, host(false));
    expect(strictProbeCounts()).toEqual({ mounts: 0, cleanups: 0 });
    await story(root, host(true));
    expect(strictProbeCounts()).toEqual({ mounts: 2, cleanups: 1 });
    expect(doubleInvokeFailure(strictProbeCounts())).toBeNull();
    await React.act(async () => root.unmount());
  });

  it("refuses every count but two and one", () => {
    expect(doubleInvokeFailure({ mounts: 0, cleanups: 0 })).not.toBeNull();
    expect(doubleInvokeFailure({ mounts: 1, cleanups: 0 })).not.toBeNull();
    expect(doubleInvokeFailure({ mounts: 2, cleanups: 2 })).not.toBeNull();
    expect(doubleInvokeFailure({ mounts: 4, cleanups: 2 })).not.toBeNull();
    expect(doubleInvokeFailure({ mounts: 2, cleanups: 1 })).toBeNull();
  });
});
