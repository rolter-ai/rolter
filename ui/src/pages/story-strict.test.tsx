import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import * as React from "react";
import { createRoot, type Root } from "react-dom/client";

import {
  doubleInvokeFailure,
  newStrictCounts,
  StrictLater,
  StrictProbe,
  StrictProbeScope,
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
    const counts = newStrictCounts();
    await story(
      root,
      <React.StrictMode>
        <StrictProbeScope counts={counts}>
          <StrictProbe />
        </StrictProbeScope>
      </React.StrictMode>,
    );
    expect(counts).toEqual({ mounts: 1, cleanups: 0 });
    expect(doubleInvokeFailure(counts)).toContain("did not double-invoke");
    await React.act(async () => root.unmount());
  });

  it("reads two mounts and one cleanup when the subject lands in a later commit", async () => {
    const root = await liveRoot();
    const counts = newStrictCounts();
    const host = (mounted: boolean) => (
      <React.StrictMode>
        <StrictProbeScope counts={counts}>
          <StrictLater mounted={mounted}>{null}</StrictLater>
        </StrictProbeScope>
      </React.StrictMode>
    );
    await story(root, host(false));
    expect(counts).toEqual({ mounts: 0, cleanups: 0 });
    await story(root, host(true));
    expect(counts).toEqual({ mounts: 2, cleanups: 1 });
    expect(doubleInvokeFailure(counts)).toBeNull();
    await React.act(async () => root.unmount());
  });

  it("keeps two hosts in one tree apart: each reads 2 mounts and 1 cleanup", async () => {
    const root = await liveRoot();
    const a = newStrictCounts();
    const b = newStrictCounts();
    const hosts = (mounted: boolean) => (
      <React.StrictMode>
        <StrictProbeScope counts={a}>
          <StrictLater mounted={mounted}>{null}</StrictLater>
        </StrictProbeScope>
        <StrictProbeScope counts={b}>
          <StrictLater mounted={mounted}>{null}</StrictLater>
        </StrictProbeScope>
      </React.StrictMode>
    );
    await story(root, hosts(false));
    await story(root, hosts(true));
    expect(a).toEqual({ mounts: 2, cleanups: 1 });
    expect(b).toEqual({ mounts: 2, cleanups: 1 });
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
