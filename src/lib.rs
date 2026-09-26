//! OpenTelemetry telemetry shared by the kpop AWS Lambda services
//! (contract, otac-service, privy-events).
//!
//! Each function image carries the ADOT collector as an in-image sidecar
//! ($out/opt/extensions/collector — see ../../nix/collector-extension.nix),
//! and this crate gets spans to it:
//!
//! * `Telemetry::init` — the usual stderr logging plus an OTLP/HTTP exporter
//!   to `OTEL_EXPORTER_OTLP_ENDPOINT` (the sidecar). Unset = plain logging,
//!   exactly as before OpenTelemetry existed.
//! * `Telemetry::instrument_request` — one root span per invocation, parented
//!   to the invocation's X-Ray context, flushed before the response returns.
//! * `xray` — the `_X_AMZN_TRACE_ID` parser that makes the parenting (and the
//!   `Sampled` cost decision) work.
//!
//! # PII discipline
//!
//! Span attributes and events follow the same rule as log lines: never emails,
//! names, raw bodies, codes, pins, tokens or keys. Root-span attributes are
//! allowlisted by `RequestAttrs`; anything added to other spans must be
//! reviewed against the same rule (spans are queryable in X-Ray exactly like
//! logs are in CloudWatch).

mod telemetry;
pub mod xray;

pub use telemetry::{LogFormat, RequestAttrs, Telemetry};

/// Re-exports so service workspaces unify versions through this crate.
pub use tracing;
pub use tracing_subscriber;
