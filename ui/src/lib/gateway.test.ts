import { afterEach, beforeEach, describe, expect, it } from "bun:test";

import { getPlaygroundKey, realtimeUrl, setPlaygroundKey } from "@/lib/gateway";

const KEY_STORAGE = "rolter.playground.key";

/** a localStorage that answers, and one that throws the way private mode does */
function stubStorage(store: Map<string, string> | "throws") {
  const api =
    store === "throws"
      ? {
          getItem() {
            throw new Error("denied");
          },
          setItem() {
            throw new Error("denied");
          },
          removeItem() {
            throw new Error("denied");
          },
        }
      : {
          getItem: (k: string) => store.get(k) ?? null,
          setItem: (k: string, v: string) => void store.set(k, v),
          removeItem: (k: string) => void store.delete(k),
        };
  // plain assignment, not defineProperty: bun runs every test file in one
  // process and another file installs this global as non-configurable, so
  // redefining it throws depending on file order
  globalThis.localStorage = api as unknown as Storage;
}

const originalStorage = globalThis.localStorage;
const originalLocation = globalThis.location;

beforeEach(() => {
  stubStorage(new Map());
});

afterEach(() => {
  globalThis.localStorage = originalStorage;
  if (originalLocation) globalThis.location = originalLocation;
});

describe("the playground key", () => {
  it("round-trips through storage", () => {
    const store = new Map<string, string>();
    stubStorage(store);
    setPlaygroundKey("sk-rolter-abc");
    expect(store.get(KEY_STORAGE)).toBe("sk-rolter-abc");
    expect(getPlaygroundKey()).toBe("sk-rolter-abc");
  });

  it("is empty when nothing is stored", () => {
    stubStorage(new Map());
    expect(getPlaygroundKey()).toBe("");
  });

  it("clears the entry rather than storing an empty key", () => {
    const store = new Map([[KEY_STORAGE, "sk-old"]]);
    stubStorage(store);
    setPlaygroundKey("");
    expect(store.has(KEY_STORAGE)).toBe(false);
    expect(getPlaygroundKey()).toBe("");
  });

  it("survives a storage that throws, so the playground still loads", () => {
    stubStorage("throws");
    expect(() => setPlaygroundKey("sk-rolter-abc")).not.toThrow();
    expect(getPlaygroundKey()).toBe("");
  });
});

describe("realtimeUrl", () => {
  const withLocation = (protocol: string, host: string) => {
    globalThis.location = { protocol, host } as unknown as Location;
  };

  it("upgrades an https page to wss and carries the key as a query param", () => {
    stubStorage(new Map([[KEY_STORAGE, "sk-rolter-abc"]]));
    withLocation("https:", "rolter.example:8443");
    const url = new URL(realtimeUrl("gpt-4o-realtime"));
    expect(url.protocol).toBe("wss:");
    expect(url.host).toBe("rolter.example:8443");
    expect(url.pathname).toBe("/gw/v1/realtime");
    expect(url.searchParams.get("model")).toBe("gpt-4o-realtime");
    expect(url.searchParams.get("api_key")).toBe("sk-rolter-abc");
  });

  it("stays on ws for a plain http page", () => {
    stubStorage(new Map());
    withLocation("http:", "localhost:5173");
    expect(realtimeUrl("m")).toStartWith("ws://localhost:5173/gw/v1/realtime?");
  });

  it("omits api_key entirely when no key is set", () => {
    stubStorage(new Map());
    withLocation("http:", "localhost:5173");
    expect(new URL(realtimeUrl("m")).searchParams.has("api_key")).toBe(false);
  });

  it("escapes a model name that would otherwise break the query", () => {
    stubStorage(new Map());
    withLocation("http:", "localhost:5173");
    const url = new URL(realtimeUrl("vendor/model?x=1&y=2"));
    expect(url.searchParams.get("model")).toBe("vendor/model?x=1&y=2");
  });
});
