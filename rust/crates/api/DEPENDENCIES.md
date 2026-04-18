# Dependencies for `api` crate

| Dependency | Version / Path | Features |
|------------|----------------|----------|
| `async-stream` | `0.3.6` | - |
| `futures` | `0.3.32` | - |
| `futures-util` | `0.3.32` | - |
| `reqwest` | `0.12` | `json`, `rustls-tls` (default-features: false) |
| `runtime` | `../runtime` | - |
| `serde` | `1` | `derive` |
| `serde_json` | `1` (workspace) | - |
| `telemetry` | `../telemetry` | - |
| `tokio` | `1` | `io-util`, `macros`, `net`, `rt-multi-thread`, `time` |
| `uuid` | `1.23.1` | `v4` |

## Dev Dependencies
| Dependency | Version | Features |
|------------|---------|----------|
| `criterion` | `0.5` | `html_reports` |
