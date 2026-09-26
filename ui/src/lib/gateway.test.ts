import { afterEach, describe, expect, it } from "bun:test";

import {
  GatewayError,
  awaitingMintedKey,
  fetchGatewayModels,
  getPlaygroundKey,
  getPlaygroundKeyState,
  keyPropagationDelay,
  realtimeUrl,
  setKeyPropagationForTests,
  setPlaygroundKey,
  subscribePlaygroundKey,
} from "@/lib/gateway";

const originalStorage = globalThis.localStorage;
const originalLocation = globalThis.location;
const originalFetch = globalThis.fetch;

afterEach(() => {
  // the key store is a module variable now, so a key one test set is a key
  // every later test in this process inherits
  setPlaygroundKey("");
  setKeyPropagationForTests(null);
  globalThis.localStorage = originalStorage;
  globalThis.fetch = originalFetch;
  // bun has no `location` of its own, so "restore" means take the stub away
  // again — left behind, it is a fake origin every later test file inherits
  if (originalLocation) globalThis.location = originalLocation;
  else delete (globalThis as { location?: Location }).location;
});

describe("the playground key", () => {
  it("round-trips in memory", () => {
    setPlaygroundKey("sk-rolter-abc");
    expect(getPlaygroundKey()).toBe("sk-rolter-abc");
  });

  it("is empty until one is set", () => {
    expect(getPlaygroundKey()).toBe("");
  });

  /**
   * The point of #944: a gateway credential in `localStorage` outlives the
   * sitting that needed it and stays in the browser until somebody clears it.
   * Asserting the stored value is gone would pass against code that writes a
   * different key, so this asserts nothing is written at all.
   */
  it("never touches localStorage", () => {
    const writes: string[] = [];
    const store = new Map<string, string>();
    globalThis.localStorage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => {
        writes.push(k);
        store.set(k, v);
      },
      removeItem: (k: string) => {
        writes.push(k);
        store.delete(k);
      },
    } as unknown as Storage;

    setPlaygroundKey("sk-rolter-abc", { expiresAt: "2026-01-01T00:30:00Z", minted: true });
    getPlaygroundKey();
    setPlaygroundKey("");

    expect(writes).toEqual([]);
  });

  it("clears the key, and its expiry with it", () => {
    setPlaygroundKey("sk-old", { expiresAt: "2026-01-01T00:30:00Z", minted: true });
    setPlaygroundKey("");
    expect(getPlaygroundKeyState()).toEqual({ key: "", expiresAt: null, minted: false });
  });

  // a pasted key has no expiry the dashboard chose, so it must not inherit the
  // one the minted key it replaced carried
  it("drops the previous expiry when a pasted key replaces a minted one", () => {
    setPlaygroundKey("sk-minted", { expiresAt: "2026-01-01T00:30:00Z", minted: true });
    setPlaygroundKey("sk-pasted");
    expect(getPlaygroundKeyState()).toEqual({
      key: "sk-pasted",
      expiresAt: null,
      minted: false,
    });
  });

  // the screen renders off this store through `useSyncExternalStore`, so a
  // write nobody is told about is a key the model picker never re-fetches with
  it("tells subscribers about a change, and stops once they leave", () => {
    let calls = 0;
    const unsubscribe = subscribePlaygroundKey(() => {
      calls += 1;
    });
    setPlaygroundKey("sk-rolter-abc");
    expect(calls).toBe(1);
    unsubscribe();
    setPlaygroundKey("sk-rolter-def");
    expect(calls).toBe(1);
  });
});

describe("realtimeUrl", () => {
  const withLocation = (protocol: string, host: string) => {
    globalThis.location = { protocol, host } as unknown as Location;
  };

  it("upgrades an https page to wss and carries the key as a query param", () => {
    setPlaygroundKey("sk-rolter-abc");
    withLocation("https:", "rolter.example:8443");
    const url = new URL(realtimeUrl("gpt-4o-realtime"));
    expect(url.protocol).toBe("wss:");
    expect(url.host).toBe("rolter.example:8443");
    expect(url.pathname).toBe("/gw/v1/realtime");
    expect(url.searchParams.get("model")).toBe("gpt-4o-realtime");
    expect(url.searchParams.get("api_key")).toBe("sk-rolter-abc");
  });

  it("stays on ws for a plain http page", () => {
    setPlaygroundKey("");
    withLocation("http:", "localhost:5173");
    expect(realtimeUrl("m")).toStartWith("ws://localhost:5173/gw/v1/realtime?");
  });

  it("omits api_key entirely when no key is set", () => {
    setPlaygroundKey("");
    withLocation("http:", "localhost:5173");
    expect(new URL(realtimeUrl("m")).searchParams.has("api_key")).toBe(false);
  });

  it("escapes a model name that would otherwise break the query", () => {
    setPlaygroundKey("");
    withLocation("http:", "localhost:5173");
    const url = new URL(realtimeUrl("vendor/model?x=1&y=2"));
    expect(url.searchParams.get("model")).toBe("vendor/model?x=1&y=2");
  });
});

/**
 * A key minted a moment ago answers 401 until the gateway's next snapshot poll
 * picks it up (#1853). These pin what gets waited out, for how long, and what
 * falls straight through to the fallback.
 */
describe("waiting for a minted key to reach the gateway", () => {
  const unauthorized = new GatewayError("invalid key", 401);

  it("reports the status a refusal came back with", async () => {
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ error: { message: "invalid key" } }), {
        status: 401,
      })) as unknown as typeof fetch;
    const error = await fetchGatewayModels().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(GatewayError);
    expect((error as GatewayError).status).toBe(401);
    expect((error as GatewayError).message).toBe("invalid key");
  });

  it("backs off by doubling, up to a ceiling", () => {
    expect([0, 1, 2, 3, 4, 5].map(keyPropagationDelay)).toEqual([250, 500, 1000, 2000, 2000, 2000]);
  });

  // two of the gateway's default 5 s polls: long enough for the key to land,
  // short enough that a key the gateway never accepts still ends in a fallback
  it("keeps asking for about two snapshot poll intervals, and then stops", () => {
    let retries = 0;
    let waited = 0;
    while (awaitingMintedKey(retries, unauthorized)) {
      waited += keyPropagationDelay(retries);
      retries += 1;
    }
    expect(retries).toBeGreaterThan(3);
    expect(waited).toBeGreaterThan(5_000);
    expect(waited).toBeLessThanOrEqual(10_000);
  });

  // anything but a 401 says something about the gateway, not about the key,
  // so waiting it out would only hold back the fallback
  it("does not wait out a failure the key cannot explain", () => {
    expect(awaitingMintedKey(0, new GatewayError("bad gateway", 502))).toBe(false);
    expect(awaitingMintedKey(0, new GatewayError("forbidden", 403))).toBe(false);
    expect(awaitingMintedKey(0, new TypeError("Failed to fetch"))).toBe(false);
  });

  it("takes a shorter budget from a story, and gives it back", () => {
    setKeyPropagationForTests({ budgetMs: 100, firstDelayMs: 50, maxDelayMs: 50 });
    expect(awaitingMintedKey(1, unauthorized)).toBe(true);
    expect(awaitingMintedKey(2, unauthorized)).toBe(false);
    setKeyPropagationForTests(null);
    expect(keyPropagationDelay(0)).toBe(250);
  });
});
