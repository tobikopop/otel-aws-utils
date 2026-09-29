# otel-aws-utils

OpenTelemetry telemetry for AWS Lambda functions written in Rust:
process-wide `tracing` setup that preserves the function's existing log
format, one root span per invocation parented to the AWS X-Ray trace, and
OTLP export to an in-image collector sidecar (the ADOT Collector Lambda
extension).

## Usage

```toml
otel-aws-utils = { git = "ssh://git@github.com/tobikopop/otel-aws-utils.git" }
```

```rust
let telemetry = Arc::new(Telemetry::init("my-service", LogFormat::LambdaDefault));
// per request:
telemetry
    .instrument_request(RequestAttrs::http("POST", "/handle", &request_id), handler(req))
    .await
```

* `Telemetry::init(service_name, format)` — installs the function's usual
  logging (`LogFormat::LambdaDefault` = `lambda_http`'s default subscriber
  behaviour; `LogFormat::JsonStderr(level)` = JSON lines on stderr) plus an
  OTLP/HTTP exporter when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. When the
  variable is unset or empty, the subscriber is exactly the pre-OpenTelemetry
  behaviour and no exporter machinery starts.
* `Telemetry::instrument_request` — one root span per invocation, parented
  to `_X_AMZN_TRACE_ID` (X-Ray), flushed before the response returns, error
  status recorded on `Err`.
* `xray` — the `_X_AMZN_TRACE_ID` parser (X-Ray's `Sampled` flag is the
  authoritative sampling decision; unsampled invocations record nothing —
  the metered X-Ray dimension, and the primary cost lever).

## PII discipline

Span attributes and events obey the same rule as log lines: never emails,
names, raw bodies, codes, pins, tokens or keys. Root-span attributes are
allowlisted by `RequestAttrs`; anything else must be reviewed against the
same rule.

## Development

`devenv shell` (toolchain pinned by `rust-toolchain.toml`), then
`devenv shell check` (fmt + clippy `-D warnings` + tests).
