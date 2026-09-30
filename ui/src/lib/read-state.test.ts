import { describe, expect, it } from "bun:test";
import {
  onlineManager,
  QueryClient,
  QueryObserver,
  type QueryObserverResult,
} from "@tanstack/react-query";

import { isAwaiting, isEmptyAnswer } from "@/lib/read-state";

// the states are read off a real observer rather than written out by hand, so
// the predicates are held to what react-query actually reports and not to a
// fixture that agrees with them by construction
function observe(
  queryFn: () => Promise<unknown[]>,
  options: { enabled?: boolean } = {},
): { read: () => QueryObserverResult<unknown[]>; settle: () => Promise<void>; stop: () => void } {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const observer = new QueryObserver(client, { queryKey: ["rows"], queryFn, ...options });
  let settled: () => void = () => {};
  const done = new Promise<void>((resolve) => (settled = resolve));
  const unsubscribe = observer.subscribe((result) => {
    if (result.fetchStatus === "idle") settled();
  });
  return {
    read: () => observer.getCurrentResult(),
    settle: () => done,
    stop: () => {
      unsubscribe();
      client.clear();
    },
  };
}

describe("isEmptyAnswer", () => {
  it("is true only for a read that succeeded with no rows", async () => {
    const q = observe(async () => []);
    await q.settle();
    expect(isEmptyAnswer(q.read(), 0)).toBe(true);
    // a search that filtered every row out is still an answer, just a narrower one
    expect(isEmptyAnswer(q.read(), 3)).toBe(false);
    q.stop();
  });

  it("is false while the read is in flight", () => {
    const q = observe(() => new Promise(() => {}));
    expect(q.read().fetchStatus).toBe("fetching");
    expect(isEmptyAnswer(q.read(), 0)).toBe(false);
    q.stop();
  });

  // the #2211 case: a failed read holds no rows, and used to read as none
  it("is false after the read failed", async () => {
    const q = observe(async () => {
      throw new Error("boom");
    });
    await q.settle();
    expect(q.read().isError).toBe(true);
    expect(isEmptyAnswer(q.read(), 0)).toBe(false);
    q.stop();
  });

  it("is false for a query that is disabled", () => {
    const q = observe(async () => [], { enabled: false });
    expect(isEmptyAnswer(q.read(), 0)).toBe(false);
    q.stop();
  });
});

describe("isAwaiting", () => {
  it("is true while the read is in flight", () => {
    const q = observe(() => new Promise(() => {}));
    expect(isAwaiting(q.read())).toBe(true);
    q.stop();
  });

  // the #1984 window: offline, a first attempt parks instead of failing, so the
  // query is pending without fetching and `isLoading` reads false
  it("is true for a read parked while offline", () => {
    onlineManager.setOnline(false);
    try {
      const q = observe(async () => []);
      expect(q.read().fetchStatus).toBe("paused");
      expect(q.read().isLoading).toBe(false);
      expect(isAwaiting(q.read())).toBe(true);
      q.stop();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("is false for a query that is disabled, so no skeleton waits on it forever", () => {
    const q = observe(async () => [], { enabled: false });
    expect(q.read().isPending).toBe(true);
    expect(isAwaiting(q.read())).toBe(false);
    q.stop();
  });

  it("is false once the read has answered, either way", async () => {
    const ok = observe(async () => []);
    await ok.settle();
    expect(isAwaiting(ok.read())).toBe(false);
    ok.stop();
    const failed = observe(async () => {
      throw new Error("boom");
    });
    await failed.settle();
    expect(isAwaiting(failed.read())).toBe(false);
    failed.stop();
  });
});
