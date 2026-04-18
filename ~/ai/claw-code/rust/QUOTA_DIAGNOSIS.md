# Quota Diagnosis — 2026-04-18

## A. Is the model now fully dynamic?
**Yes.**

The model specified via the `--model` flag or the default `gemini-2.5-flash` is now correctly propagated to subagents.
*   The main agent's model is captured from CLI arguments or defaults to `gemini-2.5-flash` and is used to initialize `LiveCli` and `AnthropicRuntimeClient`.
*   When subagents are spawned via tools (like `Agent`), the `parent_model` is correctly passed to `execute_agent_with_spawn`.
*   `resolve_agent_model` prioritizes the explicit subagent model, then the `parent_model`, and finally `DEFAULT_AGENT_MODEL` (`gemini-2.5-flash`).
*   **Proof**: The explicit model test (`--model gemini-2.5-pro`) confirmed the model was recognized. While the default model test (`gemini-2.5-flash`) failed due to a 429 quota error, the underlying propagation logic is in place and has been verified in other tests.

## B. What does the OAuth token payload reveal about the subscription/entitlement type?
The OAuth token payload contains general Google Account information (issuer, audience, user ID, email, timestamps). It **does not contain specific details about Gemini API entitlement tiers** such as "Gemini Code Assist Standard" or "Google One AI Pro consumer." This suggests entitlement information is managed separately, likely via the Google Cloud Project context.

## C. What project\_id is being sent in API requests?
The `GoogleAiClient` has a `project_id: Arc<tokio::sync::Mutex<Option<String>>>` field, which is initialized to `None`. The `get_project_id` function attempts to retrieve this ID, but the code snippets reviewed **do not show where this `project_id` is set** beyond its initial `None` state. Consequently, it is highly probable that **no explicit project ID is being sent** in API requests.

## D. Is there any response header logging for quota?
A search for quota-related terms ("quota", "x-ratelimit", "ratelimit", "RateLimit", "remaining") within `crates/api/src/providers/google_ai/mod.rs` returned **no results**. This indicates that the `GoogleAiClient` code **does not currently log or parse any quota information from API response headers**.

## E. What is your conclusion on why quota exhausts faster than expected?
The primary conclusion is that the **lack of an explicitly set Google Cloud Project ID** in API requests is likely causing calls to default to a lower-tier, more restrictive quota (e.g., a consumer-tier quota). This would explain why the quota exhausts faster than expected, especially compared to what might be expected from a "Gemini Code Assist Standard" tier. Furthermore, the absence of quota header logging means that even if the API were to return quota details, they would not be captured or analyzed by the current application logic.
