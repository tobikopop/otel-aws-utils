//! AWS X-Ray trace-header handling for Lambda.
//!
//! With active tracing enabled, the Lambda platform exposes the invocation's
//! trace context in the _X_AMZN_TRACE_ID environment variable (the same
//! X-Amzn-Trace-Id header format API Gateway passes around). Example:
//! Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8;Sampled=1
//!
//! Parenting our root span to that context is what merges application spans
//! into the invocation's X-Ray trace (instead of a disconnected trace), and
//! the Sampled flag is the authoritative sampling decision — honoring it is
//! the main X-Ray cost lever.
//!
//! The parser is deliberately local (opentelemetry-aws's XrayPropagator is a
//! moving target across opentelemetry versions): the format is tiny and
//! frozen.

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};

/// Extracts the remote parent context from the X-Amzn-Trace-Id header
/// format. Returns None for absent/malformed headers — a request without a
/// valid parent still gets a root span, it is just not merged into an
/// invocation trace.
///
/// Sampled=1 / Sampled=0 map to the sampled flag; Sampled=? (deferred) leaves
/// the decision to us and is treated as sampled.
#[must_use]
pub fn context_from_header(header: &str) -> Option<Context> {
    let mut trace_id: Option<[u8; 16]> = None;
    let mut span_id: Option<[u8; 8]> = None;
    // None = deferred ("Sampled=?") or absent: the decision is ours -> sampled.
    let mut sampled: Option<bool> = None;

    for part in header.split(';') {
        let mut kv = part.trim().splitn(2, '=');
        let (key, value) = (kv.next()?, kv.next()?);
        match key {
            "Root" => trace_id = parse_trace_id(value.trim()),
            "Parent" => span_id = parse_span_id(value.trim()),
            "Sampled" => {
                sampled = match value.trim() {
                    "1" | "true" => Some(true),
                    "0" | "false" => Some(false),
                    _ => None,
                }
            }
            _ => {}
        }
    }

    let trace_id = trace_id?;
    let span_id = span_id?;
    // All-zero ids are invalid in the trace model.
    if trace_id == [0u8; 16] || span_id == [0u8; 8] {
        return None;
    }
    let span_context = SpanContext::new(
        TraceId::from_bytes(trace_id),
        SpanId::from_bytes(span_id),
        if sampled.unwrap_or(true) {
            TraceFlags::SAMPLED
        } else {
            TraceFlags::default()
        },
        true,
        TraceState::default(),
    );
    Some(Context::new().with_remote_span_context(span_context))
}

/// context_from_header for the platform's per-invocation env var.
#[must_use]
pub fn context_from_env() -> Option<Context> {
    let raw = std::env::var("_X_AMZN_TRACE_ID").ok()?;
    context_from_header(&raw)
}

/// Root=1-<8 hex>-<24 hex> -> the 16 trace-id bytes (epoch + random).
fn parse_trace_id(root: &str) -> Option<[u8; 16]> {
    let mut parts = root.splitn(3, '-');
    let (version, epoch, random) = (parts.next()?, parts.next()?, parts.next()?);
    if version != "1" || epoch.len() != 8 || random.len() != 24 {
        return None;
    }
    let hex = format!("{epoch}{random}");
    let bytes = decode_hex(&hex)?;
    bytes.try_into().ok()
}

/// Parent=<16 hex> -> the 8 span-id bytes.
fn parse_span_id(parent: &str) -> Option<[u8; 8]> {
    let bytes = decode_hex(parent)?;
    bytes.try_into().ok()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8;Sampled=1";

    #[test]
    fn a_full_header_yields_a_sampled_remote_parent() {
        let cx = context_from_header(HEADER).expect("header parses");
        let sc = cx.span().span_context().clone();
        assert!(sc.is_valid());
        assert!(sc.is_sampled());
        assert_eq!(
            sc.trace_id().to_string(),
            "67891233abcdef012345678912345678"
        );
        assert_eq!(sc.span_id().to_string(), "53995c3f42cd8ad8");
    }

    #[test]
    fn sampled_zero_is_honored() {
        let cx = context_from_header(
            "Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8;Sampled=0",
        )
        .expect("header parses");
        assert!(!cx.span().span_context().is_sampled());
    }

    #[test]
    fn a_deferred_decision_is_treated_as_sampled() {
        let cx = context_from_header(
            "Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8;Sampled=?",
        )
        .expect("header parses");
        assert!(cx.span().span_context().is_sampled());
    }

    #[test]
    fn a_missing_or_malformed_header_yields_no_parent() {
        assert!(context_from_header("").is_none());
        assert!(context_from_header("Root=garbage").is_none());
        assert!(context_from_header("Parent=53995c3f42cd8ad8").is_none());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let cx = context_from_header(&format!("Foo=bar;{HEADER};Extra=1")).expect("header parses");
        assert!(cx.span().span_context().is_valid());
    }

    #[test]
    fn zero_trace_id_is_rejected() {
        assert!(
            context_from_header(
                "Root=1-00000000-000000000000000000000000;Parent=53995c3f42cd8ad8;Sampled=1"
            )
            .is_none()
        );
    }
}
