import * as React from "react";

// A StrictMode host for stories that proves the double-invoke happened (#1887).
//
// A story that wraps its subject in `<React.StrictMode>` inside `render` puts
// the StrictMode in the same commit as the subject. Storybook mounts every
// story under its own `ErrorBoundary`, React only doubles what sits below a
// `StrictMode` it passed on the way down from the newly placed fiber, and that
// fiber is the boundary — so nothing is doubled and the story asserts against
// a lifecycle that never ran (#1743, #1744). Nothing about the source tells
// the two shapes apart, so this checks it at runtime: `StrictModeHost` mounts
// the StrictMode first and the subject into it later, and the probe placed
// beside the subject counts the effect lifecycle React actually ran.
//
// Kept apart from `story-harness.tsx` so a `bun test` unit test can import it:
// that file pulls in `storybook/test`, which needs a browser. The waiting half,
// `expectDoubleInvoked` and `mountStrictly`, lives there. Not a `.stories.tsx`
// file, so `check:literals` skips it like the other harnesses.

/** What the probe's effect saw since the last reset. */
export interface DoubleInvokeCounts {
  mounts: number;
  cleanups: number;
}

// module state rather than a context: one subject is mounted strictly per
// story, and a story reads the counts from its play function, outside the tree
let counts: DoubleInvokeCounts = { mounts: 0, cleanups: 0 };

/** Forget earlier mounts, so a story never reads the one that ran before it. */
export function resetStrictProbe(): void {
  counts = { mounts: 0, cleanups: 0 };
}

/** A copy of the counts, so a caller cannot move them. */
export function strictProbeCounts(): DoubleInvokeCounts {
  return { ...counts };
}

/**
 * Why `counts` is not a double-invoke, or `null` when it is.
 *
 * StrictMode mounts a newly placed effect, runs its cleanup and mounts it again,
 * so a subject that is up reads exactly two mounts and one cleanup. One and
 * zero is the same-commit shape; anything else is a probe nobody reset, or a
 * subject that came and went before it was asked.
 */
export function doubleInvokeFailure({ mounts, cleanups }: DoubleInvokeCounts): string | null {
  if (mounts === 2 && cleanups === 1) return null;
  const read = `the probe read ${mounts} mount(s) and ${cleanups} cleanup(s), not 2 and 1`;
  if (mounts === 1 && cleanups === 0) {
    return (
      `StrictMode did not double-invoke the subject: ${read}. ` +
      "A StrictMode placed in the same commit as its subject doubles nothing; " +
      "mount the subject later, into a StrictModeHost (docs/dev-docs/development/testing.md)"
    );
  }
  return `StrictMode double-invoke not observed: ${read}`;
}

/** Counts its own effect's mounts and cleanups. Renders nothing. */
export function StrictProbe(): null {
  React.useEffect(() => {
    counts.mounts += 1;
    return () => {
      counts.cleanups += 1;
    };
  }, []);
  return null;
}

/**
 * The subject and its probe, placed together only once `mounted` turns true.
 * Render it under a `StrictMode` that is already mounted; `StrictModeHost` is
 * the story-facing form of exactly that.
 */
export function StrictLater({
  mounted,
  children,
}: {
  mounted: boolean;
  children: React.ReactNode;
}) {
  if (!mounted) return null;
  return (
    <>
      <StrictProbe />
      {children}
    </>
  );
}

/** The button `StrictModeHost` renders until the subject is mounted. */
export const STRICT_MOUNT_LABEL = "mount the subject";

function Mounter({ label, children }: { label: string; children: React.ReactNode }) {
  const [mounted, setMounted] = React.useState(false);
  return (
    <>
      {!mounted && (
        <button
          type="button"
          onClick={() => {
            resetStrictProbe();
            setMounted(true);
          }}
        >
          {label}
        </button>
      )}
      <StrictLater mounted={mounted}>{children}</StrictLater>
    </>
  );
}

/**
 * A `StrictMode` that is up before its subject is: `render` returns this with
 * the subject as `children`, and the play function mounts it with
 * `mountStrictly()` from `story-harness.tsx`, which fails the story unless the
 * probe saw the double-invoke. This is also the app's own shape — `main.tsx`
 * makes the root strict long before anything inside it mounts.
 */
export function StrictModeHost({
  label = STRICT_MOUNT_LABEL,
  children,
}: {
  label?: string;
  children: React.ReactNode;
}) {
  return (
    <React.StrictMode>
      <Mounter label={label}>{children}</Mounter>
    </React.StrictMode>
  );
}
