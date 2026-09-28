import { describe, it, expect, mock, beforeEach, afterEach } from "bun:test";
import {
  fetchConfig,
  createOrg,
  fetchAnalyticsSummary,
  fetchMcpSummary,
  fetchInvocationsPage,
  AnalyticsUnavailableError,
  ApiError,
  isOpenModeNoSession,
  login,
  fetchMe,
  setSessionExpiredHandler,
  apiBaseDoublesV1,
  resolveUpstreamUrl,
  isConvertible,
  unservedRoutes,
  type CurrencySettings,
} from "./api";

// We need to mock global fetch and localStorage
describe("api client", () => {
  let fetchMock: any;
  let localStorageMock: Record<string, string>;

  beforeEach(() => {
    fetchMock = mock();
    globalThis.fetch = fetchMock;

    localStorageMock = {};
    const getItemMock = mock((key: string) => localStorageMock[key] || null);
    const setItemMock = mock((key: string, val: string) => {
      localStorageMock[key] = val;
    });

    Object.defineProperty(globalThis, "localStorage", {
      value: {
        getItem: getItemMock,
        setItem: setItemMock,
      },
      writable: true,
    });
  });

  afterEach(() => {
    mock.restore();
  });

  describe("getJson (via fetchConfig)", () => {
    it("should make a GET request with auth headers if token exists", async () => {
      localStorageMock["rolter.session.token"] = "test-token";
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ providers: [], routes: [], virtual_keys: [] }), {
          status: 200,
        }),
      );

      const result = await fetchConfig();
      expect(result).toEqual({ providers: [], routes: [], virtual_keys: [] });
      expect(fetchMock).toHaveBeenCalledTimes(1);

      const callArgs = fetchMock.mock.calls[0];
      expect(callArgs[0]).toBe("/api/v1/config");
      expect(callArgs[1].headers).toEqual({
        Authorization: "Bearer test-token",
      });
    });

    it("should make a GET request without auth headers if no token exists", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ providers: [], routes: [], virtual_keys: [] }), {
          status: 200,
        }),
      );

      await fetchConfig();
      const callArgs = fetchMock.mock.calls[0];
      expect(callArgs[1].headers).toEqual({});
    });

    it("should throw an error on non-ok status (testing apiError without json)", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response("Not Found", {
          status: 404,
        }),
      );

      await expect(fetchConfig()).rejects.toThrow("request failed: 404");
    });

    it("should surface control plane error messages", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ error: { message: "superadmin access required" } }), {
          status: 403,
        }),
      );

      const request = fetchConfig();
      await expect(request).rejects.toThrow("superadmin access required");
      await expect(request).rejects.toMatchObject({ status: 403 });
    });

    it("carries the control plane's error code so screens can branch on it", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            error: {
              message: "no local account session",
              code: "open_mode_no_session",
            },
          }),
          { status: 401 },
        ),
      );

      await expect(fetchConfig()).rejects.toMatchObject({
        status: 401,
        code: "open_mode_no_session",
      });
    });
  });

  // an open-mode 401 and an ordinary one are both 401; conflating them is the
  // bug (#942), so the predicate has to be exact about which it recognizes
  describe("isOpenModeNoSession", () => {
    it("recognizes only the open-mode code", () => {
      expect(isOpenModeNoSession(new ApiError("nope", 401, "open_mode_no_session"))).toBe(true);
      expect(isOpenModeNoSession(new ApiError("nope", 401, "unauthenticated"))).toBe(false);
      expect(isOpenModeNoSession(new ApiError("nope", 401))).toBe(false);
    });

    it("is false for anything that is not an ApiError", () => {
      // react-query hands back `unknown`; a plain Error carrying a lookalike
      // message must not light up the banner
      expect(isOpenModeNoSession(new Error("open_mode_no_session"))).toBe(false);
      expect(isOpenModeNoSession(null)).toBe(false);
      expect(isOpenModeNoSession(undefined)).toBe(false);
    });
  });

  // #1079 answers a locked account with 429 + Retry-After, and #1160 renders
  // the wait. A header the client drops is a wait the user has to guess.
  describe("Retry-After (via login)", () => {
    const refusal = (headers: Record<string, string>) =>
      new Response(
        JSON.stringify({
          error: { message: "too many", code: "too_many_attempts" },
        }),
        { status: 429, headers: { "Content-Type": "application/json", ...headers } },
      );

    it("carries the delay through as seconds", async () => {
      fetchMock.mockResolvedValueOnce(refusal({ "Retry-After": "90" }));
      const err = await login("a@b.co", "pw").catch((e) => e);
      expect(err).toBeInstanceOf(ApiError);
      expect(err.code).toBe("too_many_attempts");
      expect(err.retryAfterSeconds).toBe(90);
    });

    it("drops a header it cannot read rather than rendering NaN", async () => {
      // the http-date form is legal and rolter never sends it; a screen that
      // printed `NaN seconds` would be worse than one that says nothing
      fetchMock.mockResolvedValueOnce(refusal({ "Retry-After": "Wed, 21 Oct 2026 07:28:00 GMT" }));
      const err = await login("a@b.co", "pw").catch((e) => e);
      expect(err.retryAfterSeconds).toBeUndefined();
    });

    it("is absent when the response carries no header", async () => {
      fetchMock.mockResolvedValueOnce(refusal({}));
      const err = await login("a@b.co", "pw").catch((e) => e);
      expect(err.retryAfterSeconds).toBeUndefined();
    });
  });

  describe("sendJson (via createOrg)", () => {
    it("should make a POST request with JSON body and correct headers", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ id: "org-1", name: "Test Org" }), {
          status: 200,
        }),
      );

      const input = { name: "Test Org", slug: "test-org" };
      const result = await createOrg(input);

      expect(result.id).toBe("org-1");
      const callArgs = fetchMock.mock.calls[0];
      expect(callArgs[0]).toBe("/api/v1/orgs");
      expect(callArgs[1].method).toBe("POST");
      expect(callArgs[1].headers).toHaveProperty("Content-Type", "application/json");
      expect(callArgs[1].body).toBe(JSON.stringify(input));
    });

    it("should parse and throw control plane API errors", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ error: { message: "Invalid org name" } }), {
          status: 400,
        }),
      );

      await expect(createOrg({ name: "Bad", slug: "bad" })).rejects.toThrow("Invalid org name");
    });
  });

  describe("fetchInvocationsPage", () => {
    it("sends the cursor and never an offset (#1411)", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ data: [], next_cursor: null }), { status: 200 }),
      );
      await fetchInvocationsPage({ limit: 50, cursor: "2026-07-19 12:00:01.000|req-1" });
      const url = new URL(String(fetchMock.mock.calls[0][0]), "http://localhost");
      expect(url.searchParams.get("cursor")).toBe("2026-07-19 12:00:01.000|req-1");
      expect(url.searchParams.get("limit")).toBe("50");
      expect(url.searchParams.has("offset")).toBe(false);
    });

    it("omits the cursor for the first page", async () => {
      fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ data: [] }), { status: 200 }));
      await fetchInvocationsPage({ limit: 50 });
      expect(String(fetchMock.mock.calls[0][0])).not.toContain("cursor");
    });

    it("reads next_cursor, and a missing one as the end", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ data: [{ request_id: "a" }], next_cursor: "t|a" }), {
          status: 200,
        }),
      );
      const page = await fetchInvocationsPage();
      expect(page.next_cursor).toBe("t|a");
      expect(page.data).toHaveLength(1);

      fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ data: [] }), { status: 200 }));
      expect(await fetchInvocationsPage()).toEqual({ data: [], next_cursor: null });
    });
  });

  describe("getAnalytics (via fetchAnalyticsSummary)", () => {
    it("should fetch analytics data successfully", async () => {
      const summaryData = {
        requests: 100,
        cost_usd: 5.5,
        tokens: 10,
        prompt_tokens: 5,
        completion_tokens: 5,
        failures: 0,
        p95_latency_ms: 100,
        cached_requests: 0,
        cache_hits: 0,
        avg_latency_ms: 50,
        errors: 0,
        unpriced_requests: 0,
        unpriced_models: 0,
      };
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ data: [summaryData] }), {
          status: 200,
        }),
      );

      const result = await fetchAnalyticsSummary();
      expect(result).toEqual(summaryData);
      const callArgs = fetchMock.mock.calls[0];
      expect(callArgs[0]).toContain("/api/v1/analytics/summary");
    });

    // react-query v5 rejects a query function that resolves to `undefined`, so
    // an empty envelope has to come back as `null` or the dashboard renders the
    // load-error panel instead of zeroes (#1608)
    it("should resolve an empty envelope to null, never undefined", async () => {
      fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ data: [] }), { status: 200 }));

      const result = await fetchAnalyticsSummary();
      expect(result).toBeNull();
      expect(result).not.toBeUndefined();
    });

    // the same rule one endpoint over: the MCP logs summary is a single
    // aggregate row read straight into a query, so an empty envelope there is
    // the same outage screen on a quiet deployment (#1611)
    it("resolves an empty mcp summary envelope to null too", async () => {
      fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ data: [] }), { status: 200 }));

      const result = await fetchMcpSummary();
      expect(result).toBeNull();
      expect(result).not.toBeUndefined();
    });

    it("reads the one row an mcp summary envelope carries", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ data: [{ calls: 12, failures: 1, servers: 2 }] }), {
          status: 200,
        }),
      );

      expect(await fetchMcpSummary()).toMatchObject({ calls: 12, failures: 1 });
    });

    it("should throw AnalyticsUnavailableError on 503", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response(JSON.stringify({ error: { message: "ClickHouse down" } }), {
          status: 503,
        }),
      );

      await expect(fetchAnalyticsSummary()).rejects.toThrow(AnalyticsUnavailableError);
    });

    it("should throw AnalyticsUnavailableError on 404 by default", async () => {
      fetchMock.mockResolvedValueOnce(
        new Response("Not found", {
          status: 404,
        }),
      );

      await expect(fetchAnalyticsSummary()).rejects.toThrow(AnalyticsUnavailableError);
    });
  });
});

// #947: the base-URL field taught operators to include /v1, which for
// openai-shaped kinds doubles into /v1/v1/chat/completions and 404s.
describe("api_base resolution", () => {
  it("appends /v1 for kinds that do not carry it in the base", () => {
    expect(resolveUpstreamUrl("https://api.openai.com", false)).toBe(
      "https://api.openai.com/v1/chat/completions",
    );
  });

  it("reproduces the doubling the old placeholder caused", () => {
    // the literal base from the dogfooding report
    expect(resolveUpstreamUrl("https://gpustack.localhost/v1", false)).toBe(
      "https://gpustack.localhost/v1/v1/chat/completions",
    );
    expect(apiBaseDoublesV1("https://gpustack.localhost/v1", false)).toBe(true);
  });

  it("strips the gateway /v1 for kinds whose base carries it", () => {
    expect(resolveUpstreamUrl("https://api.mistral.ai/v1", true)).toBe(
      "https://api.mistral.ai/v1/chat/completions",
    );
    // the same spelling is correct here, so it is not flagged
    expect(apiBaseDoublesV1("https://api.mistral.ai/v1", true)).toBe(false);
  });

  it("does not double the separator on a trailing slash", () => {
    expect(resolveUpstreamUrl("https://host/", false)).toBe("https://host/v1/chat/completions");
    expect(apiBaseDoublesV1("https://host/v1/", false)).toBe(true);
  });

  it("previews nothing for an empty base", () => {
    expect(resolveUpstreamUrl("", false)).toBe("");
    expect(apiBaseDoublesV1("", false)).toBe(false);
  });

  it("does not mistake /v1beta for the version prefix", () => {
    expect(apiBaseDoublesV1("https://host/v1beta", false)).toBe(false);
  });
});

// #965: the currency chooser used to be a literal seven-code list, so a
// configured RUB was unselectable and an offered JPY was rejected on save.
describe("isConvertible", () => {
  const settings = (codes: string[]): CurrencySettings => ({
    base: codes[0],
    codes,
    rates: Object.fromEntries(codes.map((c) => [c, 1])),
  });

  it("accepts any code the deployment configured", () => {
    expect(isConvertible(settings(["USD", "RUB"]), "RUB")).toBe(true);
  });

  it("rejects a code with no rate, however familiar", () => {
    // GBP shipped in the old hardcoded list but has no rate here
    expect(isConvertible(settings(["USD", "RUB"]), "GBP")).toBe(false);
  });

  it("is not a fixed set — it follows the configured table", () => {
    expect(isConvertible(settings(["EUR"]), "USD")).toBe(false);
    expect(isConvertible(settings(["EUR", "USD"]), "USD")).toBe(true);
  });

  it("compares codes case- and whitespace-insensitively", () => {
    expect(isConvertible(settings(["USD", "RUB"]), " rub ")).toBe(true);
  });

  it("stays quiet until the settings have loaded", () => {
    // warning on every price before the table arrives would be a false alarm
    expect(isConvertible(undefined, "RUB")).toBe(true);
  });
});

// #1196: the dashboard never re-checked the stored token, and never dropped
// one the control plane had already rejected.
describe("session revalidation", () => {
  let fetchMock: any;
  let store: Record<string, string>;

  beforeEach(() => {
    fetchMock = mock();
    globalThis.fetch = fetchMock;
    store = {};
    Object.defineProperty(globalThis, "localStorage", {
      value: {
        getItem: (key: string) => store[key] ?? null,
        setItem: (key: string, val: string) => {
          store[key] = val;
        },
        removeItem: (key: string) => {
          delete store[key];
        },
      },
      writable: true,
    });
  });

  afterEach(() => {
    setSessionExpiredHandler(null);
    mock.restore();
  });

  const ME = {
    user: {
      id: "user-1",
      email: "anya@acme.co",
      is_superadmin: true,
      deactivated_at: null,
      created_at: "2026-01-01T00:00:00Z",
    },
    memberships: [
      {
        id: "m-1",
        user_id: "user-1",
        org_id: "org-1",
        team_id: null,
        project_id: null,
        role: "admin",
        source: "manual",
        created_at: "2026-01-01T00:00:00Z",
      },
    ],
  };

  const refusal = (status: number, code: string) =>
    new Response(JSON.stringify({ error: { message: "no", code } }), {
      status,
      headers: { "Content-Type": "application/json" },
    });

  it("asks /auth/me with the stored token and returns {user, memberships}", async () => {
    store["rolter.session.token"] = "sess-1";
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify(ME), { status: 200 }));

    const me = await fetchMe();
    expect(me.user.is_superadmin).toBe(true);
    expect(me.user.email).toBe("anya@acme.co");
    expect(me.memberships[0].org_id).toBe("org-1");
    expect(me.memberships[0].source).toBe("manual");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/v1/auth/me");
    expect(init.headers).toEqual({ Authorization: "Bearer sess-1" });
  });

  it("signals the shell once when a request with a token is refused", async () => {
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(401, "unauthenticated"));

    await expect(fetchMe()).rejects.toThrow();
    expect(signalled).toBe(1);
  });

  it("signals on a write too, not just a read", async () => {
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(401, "unauthenticated"));

    await expect(createOrg({ name: "n", slug: "n" })).rejects.toThrow();
    expect(signalled).toBe(1);
  });

  it("stays quiet for a wrong password", async () => {
    // a stale token in storage does not make a refused login an expired
    // session: the credentials in the request are what was rejected
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(401, "invalid_credentials"));

    await expect(login("a@b.co", "pw")).rejects.toThrow();
    expect(signalled).toBe(0);
  });

  it("stays quiet in open mode, which has no session to expire", async () => {
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(401, "open_mode_no_session"));

    await expect(fetchMe()).rejects.toThrow();
    expect(signalled).toBe(0);
  });

  it("stays quiet when the request carried no token at all", async () => {
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(401, "unauthenticated"));

    await expect(fetchMe()).rejects.toThrow();
    expect(signalled).toBe(0);
  });

  it("stays quiet on a 403, which signing in again does not fix", async () => {
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    setSessionExpiredHandler(() => {
      signalled += 1;
    });
    fetchMock.mockResolvedValueOnce(refusal(403, "forbidden"));

    await expect(fetchMe()).rejects.toThrow();
    expect(signalled).toBe(0);
  });

  it("unsubscribes, so an unmounted provider is not called", async () => {
    store["rolter.session.token"] = "sess-1";
    let signalled = 0;
    const off = setSessionExpiredHandler(() => {
      signalled += 1;
    });
    off();
    fetchMock.mockResolvedValueOnce(refusal(401, "unauthenticated"));

    await expect(fetchMe()).rejects.toThrow();
    expect(signalled).toBe(0);
  });
});

/**
 * The Playground's fallback list leaves out what the snapshot prunes (#1853),
 * and the only place that is said is `/api/v1/config/problems`, in sentences.
 * The fixtures are the exact strings `sanitize_for_snapshot` and
 * `provider_problems` write.
 */
describe("unservedRoutes", () => {
  it("reads the model out of a pruned route's line", () => {
    const routes = unservedRoutes([
      "route 'claude-sonnet-4' omitted from the snapshot: it has no target that references a known provider with a positive weight",
    ]);
    expect([...routes]).toEqual(["claude-sonnet-4"]);
  });

  // a provider's own line names the provider, not a route: the routes it
  // strands are reported separately, and those are the ones that matter here
  it("ignores every line that is not about a pruned route", () => {
    const routes = unservedRoutes([
      "provider 'openrouter-edge' omitted from the snapshot: openrouter provider 'openrouter-edge' requires api_key_env",
      "duplicate route model 'gpt-4o'",
      "route 'gpt-4o-mini' omitted from the snapshot: it has no target that references a known provider with a positive weight",
    ]);
    expect([...routes]).toEqual(["gpt-4o-mini"]);
  });

  it("keeps a model name with a slash or a quote in it whole", () => {
    const routes = unservedRoutes([
      "route 'meta-llama/Llama-3.1-8B' omitted from the snapshot: it has no target that references a known provider with a positive weight",
      "route 'o'brien' omitted from the snapshot: it has no target that references a known provider with a positive weight",
    ]);
    expect([...routes]).toEqual(["meta-llama/Llama-3.1-8B", "o'brien"]);
  });

  it("is empty before the problems are known", () => {
    expect(unservedRoutes(undefined).size).toBe(0);
    expect(unservedRoutes([]).size).toBe(0);
  });
});
