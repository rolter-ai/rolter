use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Guard returned by [`init`]; holds the OTLP tracer provider (when configured)
/// so spans are flushed to the collector on process exit.
///
/// Bind it to a named local in `main` (`let _telemetry = telemetry::init();`) —
/// binding to `_` would drop it immediately and discard buffered spans.
#[must_use = "bind the guard to a named local so spans flush on exit"]
#[derive(Default)]
pub struct TelemetryGuard {
    #[cfg(feature = "otlp")]
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.provider.take() {
            // flush any batched spans before the runtime tears down
            let _ = provider.shutdown();
        }
    }
}

/// Initialize the global tracing subscriber.
///
/// Reads the `RUST_LOG` environment variable for log filtering and falls back to
/// `info`. When the `otlp` feature is enabled (default) and an OTLP endpoint is
/// configured via the standard `OTEL_EXPORTER_OTLP_ENDPOINT` /
/// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` environment variable, spans are also
/// exported to that OpenTelemetry collector; otherwise only the stdout fmt layer
/// is installed and there is no OTLP overhead.
///
/// Safe to call more than once; subsequent calls are ignored.
pub fn init() -> TelemetryGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    #[cfg(feature = "otlp")]
    if let Some(provider) = otlp::try_build_provider() {
        use opentelemetry::trace::TracerProvider as _;
        let tracer = provider.tracer("rolter");
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init();
        return TelemetryGuard {
            provider: Some(provider),
        };
    }

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init();
    TelemetryGuard::default()
}

#[cfg(feature = "otlp")]
mod otlp {
    use opentelemetry_otlp::SpanExporter;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    /// Build an OTLP tracer provider from the standard `OTEL_*` environment, or
    /// `None` when no OTLP endpoint is configured (tracing stays stdout-only).
    ///
    /// Honours the OpenTelemetry SDK env contract: `OTEL_EXPORTER_OTLP_ENDPOINT`
    /// (or the traces-specific `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) selects the
    /// receiver, `OTEL_EXPORTER_OTLP_PROTOCOL` picks gRPC (default) vs
    /// HTTP/protobuf, `OTEL_EXPORTER_OTLP_HEADERS` carries backend auth, and
    /// `OTEL_SERVICE_NAME` names the service (default `rolter`).
    pub fn try_build_provider() -> Option<SdkTracerProvider> {
        // only wire the exporter when an endpoint is set; keeps the default path
        // (no env) allocation- and network-free
        if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
            && std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_none()
        {
            return None;
        }

        // endpoint, headers and timeout are read from the OTEL_* env by the
        // exporter builder; we only steer the transport here
        let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_default();
        let exporter = if protocol.starts_with("http") {
            SpanExporter::builder().with_http().build()
        } else {
            SpanExporter::builder().with_tonic().build()
        };
        let exporter = match exporter {
            Ok(exporter) => exporter,
            Err(err) => {
                // a misconfigured collector must not take the gateway down; log
                // and fall back to stdout-only tracing
                eprintln!("rolter: OTLP span exporter init failed, tracing stays local: {err}");
                return None;
            }
        };

        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource())
            .build();
        opentelemetry::global::set_tracer_provider(provider.clone());
        Some(provider)
    }

    fn resource() -> Resource {
        let service = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "rolter".to_string());
        Resource::builder().with_service_name(service).build()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn no_endpoint_means_no_exporter() {
            // the default path (no OTLP endpoint configured) must stay
            // exporter-free so tracing has zero network overhead
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            std::env::remove_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");
            assert!(super::try_build_provider().is_none());
        }
    }
}
