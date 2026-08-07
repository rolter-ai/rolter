import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";

import App from "@/App";
import { AuthProvider } from "@/lib/auth";
// initialises i18next as a side effect: the detected locale is applied before
// the first render, so nothing flashes english on the way to another language
import "@/lib/i18n";
import { initTelemetry } from "@/lib/telemetry";
import "@/index.css";

// browser tracing, off unless the control plane injected an OTLP endpoint into
// window.__ROLTER_CONFIG__ (#805). started before render so document-load and
// the first interactions are captured
void initTelemetry();

const queryClient = new QueryClient();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </AuthProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
