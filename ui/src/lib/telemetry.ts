/**
 * Browser tracing for the dashboard (#805).
 *
 * Continues the trace from a click in the UI through the control plane and the
 * gateway to the provider, so a slow screen and the request behind it are one
 * trace rather than two unrelated stories.
 *
 * Off unless configured. The SPA is served as static assets by the control
 * plane, so the collector URL cannot be baked in at build time — it comes from
 * a `window.__ROLTER_CONFIG__` block at runtime, and when that is absent
 * (the default) nothing is loaded, nothing is exported, and no headers change.
 *
 * Dependencies are vendored through bun like everything else in `ui/`; the
 * dashboard must keep working air-gapped, so there is no runtime CDN. They are
 * also imported dynamically, so the default build never ships them to a browser
 * that will not use them.
 */
/** Runtime configuration the control plane injects into the served HTML. */
export interface RolterRuntimeConfig {
  /** OTLP/HTTP traces endpoint, e.g. `http://localhost:4318/v1/traces`. */
  otelEndpoint?: string;
  /** Reported as `service.name`; defaults to `rolter-ui`. */
  otelServiceName?: string;
  /** Dashboard build version, reported as `service.version`. */
  version?: string;
  /**
   * True when the control plane is serving with no admin token, so every
   * request is treated as superadmin. Absent means gated — the control plane
   * only injects this field when it is open (#970).
   */
  openMode?: boolean;
  /**
   * Base URL of the user documentation site, e.g. `https://docs.example.com`.
   * Absent (the default) means this deployment has no documentation host, and
   * every screen suppresses its documentation links rather than rendering one
   * that cannot resolve — see `ui/src/lib/docs.ts` (#1164).
   */
  docsBaseUrl?: string;
}

/**
 * Whether the control plane that served this dashboard is unauthenticated.
 *
 * Read from the injected block rather than an API call: the answer cannot
 * change without a control-plane restart, and asking an open control plane
 * whether it is open would be answered by the same unauthenticated surface the
 * banner is warning about.
 */
export function isOpenMode(
  config: RolterRuntimeConfig | undefined = typeof window === "undefined"
    ? undefined
    : window.__ROLTER_CONFIG__,
): boolean {
  return config?.openMode === true;
}

declare global {
  interface Window {
    __ROLTER_CONFIG__?: RolterRuntimeConfig;
  }
}

let started = false;

/**
 * The `@opentelemetry/api` trace accessor, captured when the SDK loads.
 *
 * Held here rather than imported statically so that `currentTraceId` costs
 * nothing — and downloads nothing — on a deployment with tracing off, which is
 * the default. Typed structurally for the same reason: a static type import
 * would defeat the point of the dynamic one.
 */
let traceApi: { getActiveSpan(): { spanContext(): { traceId: string } } | undefined } | undefined;

/**
 * The trace id of the span currently in context, or `""` when tracing is off or
 * nothing is active.
 *
 * This is what lets a UX event (see `ux.ts`) join the gateway request it caused,
 * so "this screen felt slow" and "this request took 4s" are one row apart
 * rather than two systems apart.
 */
export function currentTraceId(): string {
  try {
    return traceApi?.getActiveSpan()?.spanContext().traceId ?? "";
  } catch {
    // never let correlation break the thing it is describing
    return "";
  }
}

/**
 * Start browser tracing when an endpoint is configured.
 *
 * The SDK is imported dynamically, so a deployment with telemetry off never
 * downloads or parses it — the cost of this feature really is zero there, which
 * a static import would not have given us.
 *
 * Safe to call more than once; later calls are ignored. Resolves to whether
 * tracing was actually started, which the tests assert on.
 */
export async function initTelemetry(
  config: RolterRuntimeConfig | undefined = typeof window === "undefined"
    ? undefined
    : window.__ROLTER_CONFIG__,
): Promise<boolean> {
  if (started || !config?.otelEndpoint) return false;
  // the web instrumentations touch `window`/`document` on construction, so
  // guard the non-browser case (tests, any future SSR) rather than throwing
  if (typeof window === "undefined" || typeof document === "undefined") {
    return false;
  }

  const [
    { ZoneContextManager },
    { OTLPTraceExporter },
    { registerInstrumentations },
    { DocumentLoadInstrumentation },
    { FetchInstrumentation },
    { UserInteractionInstrumentation },
    { resourceFromAttributes },
    { BatchSpanProcessor, WebTracerProvider },
    { ATTR_SERVICE_NAME, ATTR_SERVICE_VERSION },
    { trace },
  ] = await Promise.all([
    import("@opentelemetry/context-zone"),
    import("@opentelemetry/exporter-trace-otlp-http"),
    import("@opentelemetry/instrumentation"),
    import("@opentelemetry/instrumentation-document-load"),
    import("@opentelemetry/instrumentation-fetch"),
    import("@opentelemetry/instrumentation-user-interaction"),
    import("@opentelemetry/resources"),
    import("@opentelemetry/sdk-trace-web"),
    import("@opentelemetry/semantic-conventions"),
    import("@opentelemetry/api"),
  ]);

  const provider = new WebTracerProvider({
    resource: resourceFromAttributes({
      [ATTR_SERVICE_NAME]: config.otelServiceName ?? "rolter-ui",
      ...(config.version ? { [ATTR_SERVICE_VERSION]: config.version } : {}),
    }),
    spanProcessors: [new BatchSpanProcessor(new OTLPTraceExporter({ url: config.otelEndpoint }))],
  });

  // zone context manager keeps the active span across async boundaries, so a
  // fetch started inside a click handler is a child of that interaction
  provider.register({ contextManager: new ZoneContextManager() });

  registerInstrumentations({
    instrumentations: [
      new DocumentLoadInstrumentation(),
      new UserInteractionInstrumentation(),
      new FetchInstrumentation({
        // without this the browser sends no `traceparent` cross-origin and the
        // dashboard's spans never join the gateway's. same-origin is the normal
        // deployment (the control plane serves both), but a split origin is a
        // supported setup and must still correlate
        propagateTraceHeaderCorsUrls: [/.*/],
        // the exporter's own POSTs must not create spans that create more spans
        ignoreUrls: [new RegExp(escapeRegExp(config.otelEndpoint))],
      }),
    ],
  });

  traceApi = trace;
  started = true;
  return true;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Test seam: forget that telemetry was started. */
export function resetTelemetryForTests(): void {
  started = false;
  traceApi = undefined;
}
