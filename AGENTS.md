# AGENTS.md

`otel-aws-utils` — OpenTelemetry telemetry for AWS Lambda functions written
in Rust. One crate, consumed by sibling Lambda services as a git
dependency over ssh (`otel-aws-utils = { git = "ssh://git@github.com/tobikopop/otel-aws-utils.git" }`,
private repo).
Human-facing description lives in `README.md`; this file carries what an
agent needs to work in the repo: commands, code map, and the rules that
must not break.

## Commands (run inside `devenv shell`)

| Task | Command |
| --- | --- |
| Enter dev shell | `devenv shell` |
| Full check (fmt + clippy + tests) | `devenv shell check` |
| Unit tests | `cargo test --locked` |
| Lint | `cargo clippy --all-targets -- -D warnings` |
| Format | `cargo fmt --all --check` |

## Code map

- `src/lib.rs` — public surface: `Telemetry`, `RequestAttrs`, `LogFormat`,
  and the `xray` module. The PII rule is documented here.
- `src/telemetry.rs` — subscriber composition (each `LogFormat` variant
  reproduces one calling service's logging contract), the OTLP/HTTP
  exporter (gated on `OTEL_EXPORTER_OTLP_ENDPOINT`; empty = off),
  `instrument_request` (root span, X-Ray parent, force-flush before return,
  error status on `Err`), and `build_resource` (`OTEL_SERVICE_NAME`,
  `OTEL_RESOURCE_ATTRIBUTES`).
- `src/xray.rs` — the `_X_AMZN_TRACE_ID` parser with its test vectors.
- `devenv.nix`/`devenv.yaml` — dev shell (rust via `rust-toolchain.toml`);
  `rust-toolchain.toml` is the single toolchain pin.

## Conventions & rules

- **Rust**: edition 2024; `rust-version = "1.88"` is the MSRV floor for
  CONSUMERS (the oldest compiler the crate must build with), while
  `rust-toolchain.toml` pins the development toolchain. Keep the two
  concepts separate when bumping either.
- **`unsafe_code = "forbid"`**, clippy `-D warnings`, fmt clean, tests green
  — the `check` script enforces all three.
- **The OTel layer is instantiated INLINE at each subscriber stack on
  purpose**: `OpenTelemetryLayer<S, _>` names the stack's exact subscriber
  type, so a closure/helper would freeze one `S` for all three. Do not
  "deduplicate" it.
- **Version pairing is pinned**: `opentelemetry_sdk` 0.32 +
  `tracing-opentelemetry` 0.33 + `opentelemetry-otlp` 0.32. Bump them
  together and re-run the tests. `opentelemetry-aws` is deliberately NOT a
  dependency — `xray.rs` is the local parser (that crate lags opentelemetry
  releases); extend the parser instead of adding it.
- **Behaviour preservation**: `LogFormat::LambdaDefault` must keep
  replicating `lambda_http::tracing::init_default_subscriber` semantics
  (stdout; level from `AWS_LAMBDA_LOG_LEVEL` else `RUST_LOG` else INFO; no
  target, no timestamps; JSON iff `AWS_LAMBDA_LOG_FORMAT=JSON`), and
  `JsonStderr` must keep the JSON-lines contract (flattened events, no span
  decoration, stderr). Changing log output breaks consumers' log pipelines
  and CloudWatch queries.
- **Telemetry must never affect request outcomes**: exporters are async and
  fire-and-forget, init failures degrade to plain logging (stderr message
  only), and `flush` is best-effort. No telemetry failure may fail or block
  a handler.
- **Sampling semantics are load-bearing for cost**: `Sampled=1/true` =>
  record, `0/false` => drop, `?`/absent => deferred (record), all-zero
  trace/span ids rejected. The remote parent's decision is authoritative.
- **PII discipline**: span attributes/events obey the same rule as log
  lines (no emails, names, raw bodies, codes, pins, tokens, keys). Root-span
  attributes come only from `RequestAttrs`; anything added elsewhere must
  be reviewed against the same rule.

## Consumption

- Git dependency over ssh; consumers pin the commit through their `Cargo.lock`
  (that SHA is the integrity pin).
- Consumers' nix builds fetch it client-side via
  `cargoLock.allowBuiltinFetchGit = true` (honours the local ssh agent) — no
  hashes, no source-copying hacks.
- The collector sidecar the exporter talks to is a separate flake that
  builds the ADOT Collector Lambda extension; the crate only speaks OTLP to
  `OTEL_EXPORTER_OTLP_ENDPOINT` and does not depend on it.
