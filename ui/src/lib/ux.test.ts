import { describe, it, expect, mock, beforeEach, afterEach } from "bun:test";

import { UI_EVENTS_MAX_BATCH, type UiEvent } from "./api";
import {
  flush,
  pendingUxEvents,
  resetUxForTests,
  sanitizeKey,
  setUxContext,
  track,
  trackErrorState,
  trackFormAbandon,
  trackRefusedClick,
  trackRetrySubmit,
  trackScreenView,
  trackValidationError,
} from "./ux";

/** The batch a flush actually put on the wire. */
function sentBatch(fetchMock: any, call = 0): UiEvent[] {
  const body = fetchMock.mock.calls[call][1].body as string;
  return (JSON.parse(body) as { events: UiEvent[] }).events;
}

describe("ux event emitters", () => {
  let fetchMock: any;

  beforeEach(() => {
    resetUxForTests();
    fetchMock = mock(async () => new Response(null, { status: 202 }));
    globalThis.fetch = fetchMock;
    Object.defineProperty(globalThis, "localStorage", {
      value: { getItem: () => null, setItem: () => {} },
      writable: true,
    });
  });

  afterEach(() => {
    mock.restore();
  });

  describe("sanitizeKey", () => {
    it("keeps the slugs the dashboard actually uses as keys", () => {
      for (const key of ["models", "model-settings", "provider.create", "mcp:oauth", "v1_2"]) {
        expect(sanitizeKey(key)).toBe(key);
      }
    });

    it("rejects a url rather than truncating it", () => {
      // truncating would produce a plausible-looking key that is really a
      // fragment of a URL — ids, query parameters and all
      expect(sanitizeKey("https://rolter.example/models/abc-123?tab=targets")).toBe("");
    });

    it("rejects prose, which is how free text would arrive", () => {
      // the realistic mistake is passing an error message or a form value
      expect(sanitizeKey("provider key sk-live-abcdef is invalid")).toBe("");
      expect(sanitizeKey("Request failed: 500")).toBe("");
    });

    it("rejects anything past the server's key length", () => {
      expect(sanitizeKey("a".repeat(96))).toBe("a".repeat(96));
      expect(sanitizeKey("a".repeat(97))).toBe("");
    });

    it("treats empty and whitespace as absent", () => {
      expect(sanitizeKey(undefined)).toBe("");
      expect(sanitizeKey("   ")).toBe("");
    });
  });

  describe("queueing", () => {
    it("drops an event whose screen key is unusable", () => {
      // the whole batch would 400 on a bad key, so a bad event must not be
      // allowed to take good ones down with it
      trackScreenView("https://rolter.example/models?tab=1");
      expect(pendingUxEvents()).toHaveLength(0);
    });

    it("drops an unusable target but keeps the event", () => {
      trackValidationError("providers", "the api key you entered is not valid");
      const [event] = pendingUxEvents();
      expect(event.screen).toBe("providers");
      expect(event.action).toBe("validation_error");
      expect(event.target).toBeUndefined();
    });

    it("stamps every event with a session id and the scope labels", () => {
      setUxContext({ orgId: "org-1", teamId: "team-1", projectId: "proj-1" });
      trackScreenView("dashboard");
      const [event] = pendingUxEvents();
      expect(event.session_id).toBeTruthy();
      expect(event.org_id).toBe("org-1");
      expect(event.team_id).toBe("team-1");
      expect(event.project_id).toBe("proj-1");
    });

    it("stamps each event when it is queued, not when the batch flushes", async () => {
      // the bug this guards: stamping at flush time put every event of a
      // burst at the same instant, seconds away from when it happened (#1224)
      const realNow = Date.now;
      let clock = Date.parse("2026-09-18T10:00:00.000Z");
      Date.now = () => clock;
      // Date's constructor reads the real clock, not Date.now, so the stamp
      // has to come from a fake the whole test agrees on
      const RealDate = globalThis.Date;
      class FrozenDate extends RealDate {
        constructor(value?: number | string | Date) {
          super(value ?? clock);
        }
      }
      globalThis.Date = FrozenDate as unknown as DateConstructor;
      try {
        trackScreenView("dashboard");
        clock += 4_500;
        trackScreenView("logs");
        const [a, b] = pendingUxEvents();
        expect(a.ts).toBe("2026-09-18T10:00:00.000Z");
        expect(b.ts).toBe("2026-09-18T10:00:04.500Z");

        // and the gap survives the batch they share
        clock += 30_000;
        await flush();
        const sent = sentBatch(fetchMock);
        expect(sent.map((event) => event.ts)).toEqual([
          "2026-09-18T10:00:00.000Z",
          "2026-09-18T10:00:04.500Z",
        ]);
      } finally {
        globalThis.Date = RealDate;
        Date.now = realNow;
      }
    });

    it("gives every event a distinct id", () => {
      trackScreenView("dashboard");
      trackScreenView("logs");
      const [a, b] = pendingUxEvents();
      expect(a.event_id).not.toBe(b.event_id);
    });

    it("clamps a duration into the UInt32 column", () => {
      trackFormAbandon("keys", "virtual-key", -5);
      expect(pendingUxEvents()[0].duration_ms).toBe(0);

      resetUxForTests();
      trackFormAbandon("keys", "virtual-key", 9e12);
      expect(pendingUxEvents()[0].duration_ms).toBe(4_294_967_295);
    });

    it("flushes on its own once a full batch is queued", async () => {
      for (let i = 0; i < UI_EVENTS_MAX_BATCH; i += 1) trackScreenView("dashboard");
      await flush();
      expect(fetchMock).toHaveBeenCalled();
      expect(sentBatch(fetchMock)).toHaveLength(UI_EVENTS_MAX_BATCH);
    });

    it("never sends more than the server's batch limit in one request", async () => {
      for (let i = 0; i < UI_EVENTS_MAX_BATCH + 20; i += 1) trackScreenView("dashboard");
      await flush();
      expect(sentBatch(fetchMock).length).toBeLessThanOrEqual(UI_EVENTS_MAX_BATCH);
    });
  });

  describe("flushing", () => {
    it("posts the queue to the ingest endpoint and empties it", async () => {
      trackScreenView("models");
      trackErrorState("models", "catalog");
      await flush();

      expect(fetchMock.mock.calls[0][0]).toBe("/api/v1/ui-events");
      expect(fetchMock.mock.calls[0][1].method).toBe("POST");
      expect(sentBatch(fetchMock).map((e) => e.action)).toEqual(["screen_view", "error_state"]);
      expect(pendingUxEvents()).toHaveLength(0);
    });

    it("does nothing when there is nothing queued", async () => {
      await flush();
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it("swallows a server error instead of surfacing it", async () => {
      fetchMock.mockResolvedValueOnce(new Response("boom", { status: 500 }));
      trackScreenView("models");
      // a dashboard must not break because analytics did
      await flush();
      expect(pendingUxEvents()).toHaveLength(0);
    });

    it("stops trying after the endpoint refuses the session", async () => {
      // an older control plane, a proxy that drops the route, or a session that
      // cannot authenticate: retrying costs a request per flush and never works
      fetchMock.mockResolvedValueOnce(new Response("no", { status: 401 }));
      trackScreenView("models");
      await flush();
      expect(fetchMock).toHaveBeenCalledTimes(1);

      trackScreenView("logs");
      expect(pendingUxEvents()).toHaveLength(0);
      await flush();
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it("drops a batch the server rejected without disabling itself", async () => {
      // 400 is what the control plane answers when one event in the batch is
      // malformed, and it rejects the batch whole (verified end to end in
      // `crates/rolter-control/tests/ux_pipeline.rs`, #1728). so the cost of a
      // bad key is every interaction that shared its flush — but the stream
      // survives, which is why 400 must not join the terminal set
      fetchMock.mockResolvedValueOnce(new Response("bad", { status: 400 }));
      trackScreenView("models");
      await flush();
      expect(pendingUxEvents()).toHaveLength(0);

      trackScreenView("logs");
      await flush();
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });

    it("keeps trying after a transient failure", async () => {
      fetchMock.mockResolvedValueOnce(new Response("boom", { status: 503 }));
      trackScreenView("models");
      await flush();

      trackScreenView("logs");
      await flush();
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });
  });

  describe("the struggle signals", () => {
    it("splits a dirty abandon from a clean one", () => {
      // a form closed untouched is a misclick; one closed with a draft in it is
      // somebody who filled it in and gave up. two actions, not one with a flag,
      // so the existing form_abandon series keeps meaning what it always meant
      trackFormAbandon("providers", "provider-create", 400);
      trackFormAbandon("providers", "provider-create", 90_000, true);
      const [clean, gaveUp] = pendingUxEvents();
      expect(clean.action).toBe("form_abandon");
      expect(gaveUp.action).toBe("abandon_dirty");
      // both stay cancellations: the user did not submit either one
      expect(gaveUp.outcome).toBe("cancelled");
      expect(gaveUp.duration_ms).toBe(90_000);
    });

    it("records a retry as its own action, not a second first attempt", () => {
      trackRetrySubmit("providers", "provider-create", 2_500);
      const [event] = pendingUxEvents();
      expect(event.action).toBe("retry_submit");
      expect(event.target).toBe("provider-create");
      expect(event.outcome).toBe("error");
    });

    it("records a refusal as the control and the capability that refused it", () => {
      trackRefusedClick("providers", "provider-new", "provider:create");
      const [event] = pendingUxEvents();
      expect(event.action).toBe("refused_click");
      expect(event.target).toBe("provider-new:provider:create");
      expect(event.outcome).toBe("error");
    });

    it("carries no identity beyond the session and scope ids", () => {
      // this is what makes shipping it on by default defensible: the row says
      // "this permission boundary is in somebody's way", never who reached for
      // it. the user id is filled server-side from the session and nothing here
      // narrows the event to a person
      setUxContext({ orgId: "org-1", teamId: "team-1", projectId: "proj-1" });
      trackRefusedClick("providers", "provider-new", "provider:create");
      const [event] = pendingUxEvents();
      expect(Object.keys(event).sort()).toEqual(
        [
          "action",
          "event_id",
          "org_id",
          "outcome",
          "project_id",
          "screen",
          "session_id",
          "target",
          "team_id",
          "ts",
        ].sort(),
      );
    });

    it("drops a refusal whose capability is not a key", () => {
      // the capability is the whole signal, so an unusable one costs the event
      trackRefusedClick("providers", "provider-new", "you need the admin role");
      expect(pendingUxEvents()).toHaveLength(0);
    });

    it("keeps the capability when the control key is a label rather than a key", () => {
      // failing soft in this direction only: a call site that passed the
      // button's text still tells us which boundary was hit, and the label
      // itself never reaches the wire
      trackRefusedClick("providers", "Add provider", "provider:create");
      const [event] = pendingUxEvents();
      expect(event.target).toBe("provider:create");
    });

    it("falls back to the capability when the joined key would not fit", () => {
      // two keys that each fit can still be too long joined, and dropping the
      // whole target would lose the capability along with the control
      trackRefusedClick("providers", "c".repeat(90), "provider:create");
      const [event] = pendingUxEvents();
      expect(event.target).toBe("provider:create");
    });
  });

  describe("the payload the server sees", () => {
    it("carries no field a form value could travel in", async () => {
      // the guarantee this stream is built on: if a future field were added
      // that could hold free text, this is the test that should fail
      const allowed = new Set([
        "event_id",
        "screen",
        "action",
        "outcome",
        "ts",
        "target",
        "from_screen",
        "duration_ms",
        "trace_id",
        "session_id",
        "org_id",
        "team_id",
        "project_id",
        "app_version",
      ]);
      setUxContext({ orgId: "org-1", teamId: "team-1", projectId: "proj-1" });
      track("providers", "form_submit", {
        target: "provider-create",
        fromScreen: "dashboard",
        outcome: "error",
        durationMs: 1200,
      });
      await flush();

      for (const key of Object.keys(sentBatch(fetchMock)[0])) {
        expect(allowed.has(key)).toBe(true);
      }
    });

    it("never sends a user id — the server fills it from the session", async () => {
      trackScreenView("dashboard");
      await flush();
      expect(sentBatch(fetchMock)[0]).not.toHaveProperty("user_id");
    });
  });
});
