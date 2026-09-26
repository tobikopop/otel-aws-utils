//! Process-wide telemetry: the service's usual stdout/stderr logging plus
//! optional OpenTelemetry tracing to the in-image collector sidecar.
//!
//! The switch is the standard OTEL_EXPORTER_OTLP_ENDPOINT variable (Terraform
//! sets it to http://127.0.0.1:4318, the sidecar's OTLP/HTTP receiver).
//! Unset — unit tests, ministack e2e — the subscriber is exactly the service's
//! pre-OpenTelemetry logging behaviour and no exporter machinery starts.

use std::future::Future;
use std::str::FromStr;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

use crate::xray;

/// Which pre-existing logging behaviour to preserve. Each variant reproduces
/// one service's output contract (format, writer, level handling), so turning
/// tracing on never changes what lands in the log group's text lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// lambda_http::tracing::init_default_subscriber behaviour (contract,
    /// privy-webhook): plain text (or JSON when Lambda's advanced logging
    /// controls say so), to stdout, no target, no timestamps, level from
    /// AWS_LAMBDA_LOG_LEVEL else RUST_LOG else INFO.
    LambdaDefault,
    /// otac's logging.rs behaviour: JSON lines on stderr (flattened events,
    /// no span decoration), capped at the given level (from LOG_LEVEL).
    JsonStderr(LevelFilter),
}

/// The root span of one request/event. This struct is the attribute
/// allowlist: PII discipline (no emails, names, raw bodies, codes, pins,
/// tokens, keys) is enforced by only ever passing these fields.
#[derive(Debug, Clone)]
pub struct RequestAttrs {
    /// HTTP method ("POST") or the event trigger ("EVENT").
    pub method: String,
    /// URL path ("/mint") or the event type.
    pub path: String,
    /// The platform/API request id.
    pub request_id: String,
    /// faas.trigger value: "http" | "pubsub" | "timer" | "datasource"…
    pub faas_trigger: &'static str,
}

impl RequestAttrs {
    /// An HTTP request's attributes.
    #[must_use]
    pub fn http(method: &str, path: &str, request_id: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            request_id: request_id.to_string(),
            faas_trigger: "http",
        }
    }

    /// A stream/event-source invocation's attributes.
    #[must_use]
    pub fn event(event_type: &str, request_id: &str) -> Self {
        Self {
            method: "EVENT".to_string(),
            path: event_type.to_string(),
            request_id: request_id.to_string(),
            faas_trigger: "datasource",
        }
    }
}

/// The process-wide telemetry handle. Holds the tracer provider when tracing
/// is enabled, nothing otherwise.
#[derive(Debug)]
pub struct Telemetry {
    provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    /// Installs the global tracing subscriber and returns the handle.
    ///
    /// * `format` reproduces the service's existing logging (see [LogFormat]).
    /// * Tracing is enabled iff OTEL_EXPORTER_OTLP_ENDPOINT is set; exported
    ///   spans are parented to the invocation's X-Ray context and follow its
    ///   sampling decision.
    ///
    /// Safe to call more than once: a later call keeps the first subscriber
    /// instead of panicking.
    #[must_use]
    pub fn init(service_name: &str, format: LogFormat) -> Self {
        let provider = build_provider(service_name);
        // NOTE: the OTel layer is built INLINE at each stack (three sites)
        // on purpose: OpenTelemetryLayer<S, _> must name each stack's exact
        // subscriber type S, and a closure/helper would freeze one S for all.

        match format {
            LogFormat::LambdaDefault => {
                let json = std::env::var("AWS_LAMBDA_LOG_FORMAT")
                    .map(|v| v.eq_ignore_ascii_case("json"))
                    .unwrap_or(false);
                let level = std::env::var("AWS_LAMBDA_LOG_LEVEL")
                    .or_else(|_| std::env::var("RUST_LOG"))
                    .ok()
                    .and_then(|v| LevelFilter::from_str(v.trim()).ok())
                    .unwrap_or(LevelFilter::INFO);
                let filter = EnvFilter::builder()
                    .with_default_directive(level.into())
                    .from_env_lossy();
                let base = tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .without_time()
                    .with_writer(std::io::stdout);
                if json {
                    let _ = tracing_subscriber::registry()
                        .with(base.json().with_filter(filter))
                        .with(provider.as_ref().map(|p| {
                            tracing_opentelemetry::layer()
                                .with_tracer(p.tracer(service_name.to_string()))
                        }))
                        .try_init();
                } else {
                    let _ = tracing_subscriber::registry()
                        .with(base.with_filter(filter))
                        .with(provider.as_ref().map(|p| {
                            tracing_opentelemetry::layer()
                                .with_tracer(p.tracer(service_name.to_string()))
                        }))
                        .try_init();
                }
            }
            LogFormat::JsonStderr(level) => {
                let filter = EnvFilter::builder()
                    .with_default_directive(level.into())
                    .parse("")
                    .unwrap_or_else(|_| EnvFilter::new("info"));
                let _ = tracing_subscriber::registry()
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .flatten_event(true)
                            .with_current_span(false)
                            .with_span_list(false)
                            .with_target(false)
                            .with_ansi(false)
                            .with_writer(std::io::stderr)
                            .with_filter(filter),
                    )
                    .with(provider.as_ref().map(|p| {
                        tracing_opentelemetry::layer()
                            .with_tracer(p.tracer(service_name.to_string()))
                    }))
                    .try_init();
            }
        }

        Self { provider }
    }

    /// Whether spans are exported to the sidecar.
    #[must_use]
    pub fn tracing_enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// Runs `f` inside the request's root span and flushes before returning.
    ///
    /// The root span is parented to the invocation's X-Ray context
    /// (_X_AMZN_TRACE_ID), so the X-Ray console shows one tree:
    /// API Gateway -> Lambda -> this span -> its children. The flush before
    /// the response is what gets spans to the sidecar before the Lambda
    /// sandbox freezes; delivery from the sidecar to X-Ray is asynchronous
    /// and best-effort.
    ///
    /// On Err the span is marked errored with the error's Display text
    /// (typed errors only ever carry their safe text).
    pub async fn instrument_request<F, T, E>(&self, attrs: RequestAttrs, f: F) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        use tracing::Instrument;

        let name = format!("{} {}", attrs.method, attrs.path);
        let span = tracing::info_span!(
            "request",
            otel.name = %name,
            otel.status_message = tracing::field::Empty,
            "http.request.method" = %attrs.method,
            "url.path" = %attrs.path,
            "faas.trigger" = attrs.faas_trigger,
            "request_id" = %attrs.request_id,
        );
        if let Some(cx) = xray::context_from_env() {
            let _ = span.set_parent(cx);
        }

        let out = f.instrument(span.clone()).await;

        if let Err(error) = &out {
            span.record("otel.status_message", tracing::field::display(error));
            span.set_status(opentelemetry::trace::Status::error("request failed"));
        }
        self.flush();
        out
    }

    /// Pushes buffered spans to the sidecar (no-op when tracing is disabled).
    pub fn flush(&self) {
        if let Some(provider) = &self.provider {
            let _ = provider.force_flush();
        }
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Builds the tracer provider, or None when OTEL_EXPORTER_OTLP_ENDPOINT is
/// unset or the exporter cannot be built (logged to stderr, never fatal).
fn build_provider(service_name: &str) -> Option<SdkTracerProvider> {
    // Empty counts as unset (Terraform passes "" where telemetry is off).
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|v| !v.is_empty())?;
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|error| eprintln!("otel: exporter init failed: {error}"))
        .ok()?;

    let provider = SdkTracerProvider::builder()
        // ParentBased: the X-Ray Sampled flag decides whether the invocation
        // records anything at all (the metered dimension). No parent (direct
        // invoke) -> record.
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::AlwaysOn)))
        .with_batch_exporter(exporter)
        .with_resource(build_resource(service_name))
        .build();
    Some(provider)
}

/// service.name (OTEL_SERVICE_NAME overrides the compiled-in name) and
/// OTEL_RESOURCE_ATTRIBUTES (Terraform passes deployment.environment,
/// cloud.region, faas.name).
fn build_resource(service_name: &str) -> Resource {
    let name = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| service_name.to_string());
    let mut attributes = vec![KeyValue::new("service.name", name)];
    if let Ok(extra) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        for pair in extra.split(',') {
            let mut kv = pair.trim().splitn(2, '=');
            if let (Some(key), Some(value)) = (kv.next(), kv.next())
                && !key.is_empty()
                && !value.is_empty()
            {
                attributes.push(KeyValue::new(key.to_string(), value.to_string()));
            }
        }
    }
    Resource::builder().with_attributes(attributes).build()
}
