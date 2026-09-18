import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";

import App from "@/App";
import { AuthProvider } from "@/lib/auth";
import { ToastProvider } from "@/lib/toast";
import { classifyLoadError, isRetryable } from "@/lib/load-error";
// initialises i18next as a side effect: the detected locale is applied before
// the first render, so nothing flashes english on the way to another language
import "@/lib/i18n";
import { initTelemetry } from "@/lib/telemetry";
import "@/index.css";

// browser tracing, off unless the control plane injected an OTLP endpoint into
// window.__ROLTER_CONFIG__ (#805). started before render so document-load and
// the first interactions are captured
void initTelemetry();

// one retry policy for every screen: a 401, a 403 or an open-mode deployment
// cannot be retried into success, so retrying them three times with backoff
// (the library default) only delayed the LoadError by several seconds of blank
// screen. transient failures — an unreachable control plane, a 5xx — still get
// the default three attempts
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => isRetryable(classifyLoadError(error)) && failureCount < 3,
      // the org/team/project scope is read by every screen; without a stale
      // window it was refetched on every navigation before the screen's own
      // data could even start loading. mutations invalidate explicitly
      staleTime: 15_000,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <AuthProvider>
          <BrowserRouter>
            <App />
          </BrowserRouter>
        </AuthProvider>
      </ToastProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
