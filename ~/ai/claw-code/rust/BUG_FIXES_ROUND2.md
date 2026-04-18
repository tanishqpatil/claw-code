## Bug 1 — Model Not Propagating to Subagents
**Root cause found at**: `rust/crates/tools/src/lib.rs` in `resolve_agent_model()` and `build_agent_runtime()`.

**What was wrong**:
1.  The `resolve_agent_model()` function had a hardcoded fallback to `"gemini-2.5-flash"` for any Gemini parent model, completely ignoring the actual model version (e.g., `gemini-2.5-pro`) passed from the parent.
2.  The `AgentJob` struct did not have a field to store the `parent_model`. This information was lost between the agent spawning and the subagent's runtime construction.
3.  The `build_agent_runtime()` function did not use the `parent_model` and would incorrectly fall back to the global `DEFAULT_AGENT_MODEL` if the subagent's manifest didn't explicitly specify a model.

**Fix applied**:
1.  Added a `parent_model: Option<String>` field to the `AgentJob` struct.
2.  Updated `execute_agent_with_spawn()` to populate this new field from the `parent_model` argument.
3.  Updated `build_agent_runtime()` to call `resolve_agent_model()` with both the manifest model and the `job.parent_model`.
4.  Corrected `resolve_agent_model()` to return the actual `parent_model` value if it exists, removing the hardcoded logic.

**Before:**
```rust
// In resolve_agent_model()
if let Some(pm) = parent_model {
    if pm.starts_with("gemini-") || pm == "google" {
        return "gemini-2.5-flash".to_string(); // BUG: Hardcoded
    }
}
DEFAULT_AGENT_MODEL.to_string()

// In build_agent_runtime()
let model = job
    .manifest
    .model
    .clone()
    .unwrap_or_else(|| DEFAULT_AGENT_MODEL.to_string()); // BUG: Incorrect fallback
```

**After:**
```rust
// In resolve_agent_model()
if let Some(pm) = parent_model {
    return pm.to_string(); // FIX: Use the actual parent model
}
DEFAULT_AGENT_MODEL.to_string()

// In build_agent_runtime()
let model = resolve_agent_model(
    job.manifest.model.as_deref(),
    job.parent_model.as_deref(), // FIX: Pass parent model from job
);
```

## Bug 2 — No 429 Retry Logic
**Added in**: `rust/crates/api/src/providers/google_ai/mod.rs` inside the `send_message()` and `stream_message()` functions.

**Logic**: A `loop` was wrapped around the `self.client.post(...).send().await` API call.
- If the response status is `429 Too Many Requests` and the attempt count is less than 3:
  - It parses the JSON error body to find the `retryDelay` field (e.g., `"42.5s"`).
  - If the delay is over 300 seconds, it fails immediately, assuming the quota is fully exhausted.
  - Otherwise, it logs a `[CLAW RETRY]` message to `stderr` and sleeps for the specified duration before the next attempt.
- If the response is successful or another type of error occurs, the loop breaks.

## Build
**cargo build --release**: SUCCESS
**Binary size**: 17M
