# Claw Code Architecture

This document provides a comprehensive overview of the Claw Code architecture, focusing on the core Rust implementation and the integration of Google Gemini (Google AI) as a primary LLM provider.

## 1. Component Map

Claw Code is structured as a Rust workspace with several specialized crates.

### Core Crates

| Crate | Responsibility | Key Connections |
|-------|----------------|-----------------|
| `rusty-claude-cli` | Main entry point, CLI argument parsing, interactive loop, and terminal UI. | Depends on `runtime`, `tools`, `api`, `telemetry`. |
| `runtime` | The "orchestration engine". Manages conversation state, session compaction, system prompt construction, and tool permission enforcement. | Depends on `api` (for model calls), `tools` (for tool execution). |
| `api` | Unified interface for LLM providers (Anthropic, Google, OpenAI). Handles request serialization and SSE stream parsing. | Used by `runtime`. |
| `tools` | Implementation of native tools (e.g., `bash`, `read_file`, `write_file`, `Agent`). Defines the tool registry and execution logic. | Used by `runtime`. |
| `telemetry` | Tracing, logging, and session recording (stored in `.claw/sessions`). | Used by all crates. |
| `plugins` | Support for external extensions (e.g., Model Context Protocol / MCP). | Used by `runtime`. |

### Utility Crates
- `commands`: Implementation of specific CLI commands (e.g., `acp`, `system-prompt`).
- `compat-harness`: Testing framework for ensuring provider parity.
- `mock-anthropic-service`: Local mock server for integration testing without hitting real APIs.

---

## 2. Request Lifecycle

The following trace describes the flow of a single user prompt from input to response.

1. **User Input**: The user types a prompt in the `LiveCli` interactive loop (`crates/rusty-claude-cli/src/main.rs`).
2. **Turn Initiation**: `LiveCli::handle_user_input()` calls `LiveCli::send_message_and_display()`.
3. **Runtime Execution**: `ConversationRuntime::run_turn(prompt)` is invoked in `crates/runtime/src/conversation.rs`.
   - **Prompt Construction**: `SystemPromptBuilder` (`crates/runtime/src/prompt.rs`) gathers system instructions, project context (from `.claw/CLAUDE.md` or `GEMINI.md`), and environment data.
   - **Request Assembly**: `ApiRequest` is built, containing the system prompt and the full conversation history.
4. **API Dispatch**: `ProviderRuntimeClient::stream(request)` (implemented in `main.rs`) is called.
   - It delegates to `ApiProviderClient::stream_message(request)` in `crates/api/src/client.rs`.
5. **Provider Logic (Gemini Example)**:
   - `GoogleAiClient::stream_message()` in `crates/api/src/providers/google_ai/mod.rs` is called.
   - **Credential Loading**: Loads OAuth tokens from `~/.config/gcloud/` or environment.
   - **Schema Sanitization**: `sanitize_schema()` ensures tool definitions comply with Gemini's strict JSON schema requirements.
   - **HTTP Request**: A POST request is sent to the `cloudcode-pa.googleapis.com` endpoint.
6. **Streaming Response**: SSE events are received and parsed into `StreamEvent`s.
   - `GoogleAiClient` maps Gemini's response format back to Claw's internal `AssistantEvent`s (Text, ToolUse, etc.).
7. **Tool Execution (The "Agentic Loop")**:
   - If the model emits a `ToolUse` event, the `ConversationRuntime` checks permissions via `PermissionEnforcer` (`crates/runtime/src/permissions.rs`).
   - If approved, `SubagentToolExecutor::execute()` calls `tools::execute_tool()`.
   - The result is appended to the `Session`, and the loop repeats (Step 3) until the model provides a final text response.
8. **UI Update**: `LiveCli` renders text deltas to the terminal as they arrive.

---

## 3. Google AI Provider Integration Points

The Gemini integration is deeply woven into the codebase to provide a first-class experience:

- **Model Mapping**: `crates/api/src/providers/mod.rs` contains `resolve_model_alias()` which maps "gemini" to "gemini-2.5-pro" and identifies "gemini-*" prefixes as Google-owned.
- **Client Factory**: `crates/api/src/client.rs` handles the instantiation of `GoogleAiClient` within the `ProviderClient` enum.
- **Prompt Tailoring**: `crates/runtime/src/prompt.rs` detects if the active model is Gemini and injects the "Questioning Protocol" (forcing the model to plan before writing) and Gemini-specific documentation.
- **Role Normalization**: `crates/rusty-claude-cli/src/main.rs` includes logic in `convert_messages()` to merge consecutive "user" or "tool" messages, satisfying Gemini's strict requirement for alternating "user" and "model" roles.
- **Custom System Prompt**: The system prompt builder (`prompt.rs`) looks for `GEMINI.md` specifically when using a Gemini model, allowing for model-specific project instructions.

---

## 4. Known Weaknesses & Risks

1. **OAuth Session Expiry**: Gemini integration depends on external OAuth credentials. If the token expires or the credential file is missing, the agent will fail with an authentication error that might not be clearly actionable for the user.
2. **Schema Compliance**: While `sanitize_schema()` handles many issues, complex nested JSON schemas in tools (especially from MCP servers) may still trigger "Invalid Argument" errors from the Google AI API if they contain unsupported keywords (e.g., `oneOf`, `anyOf`, or empty `properties`).
3. **Tool Result Interleaving**: Gemini expects tool results to be part of a "user" message immediately following a "model" message containing tool calls. If internal state compaction or multi-agent handoffs disrupt this sequence, the API will reject the request.
---

## 5. Google AI / Gemini Implementation Details

This section details the internal mechanics of the `GoogleAiClient` located in `rust/crates/api/src/providers/google_ai/mod.rs`.

### End-to-End Authentication

Gemini integration uses Google OAuth2 credentials. The process involves three stages:

1.  **Credential Discovery**: The client looks for `~/.gemini/oauth_creds.json` (or a path specified by `GOOGLE_OAUTH_CREDS_FILE`). This file must contain an `access_token` and a `refresh_token`.
2.  **Token Refresh (`get_valid_token`)**: Before every request, the client checks if the cached `access_token` is expired or nearing expiry (within 60 seconds). If so, it performs an OAuth2 refresh flow against `https://oauth2.googleapis.com/token`.
3.  **Project ID Resolution (`get_project_id`)**: Google AI requests require a Google Cloud Project ID.
    - It first checks the `GOOGLE_CLOUD_PROJECT` environment variable.
    - If unset, it calls the `loadCodeAssist` endpoint (`https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist`). This API resolves the user's primary project based on their OAuth identity.

### Request Translation (`translate_request`)

The generic `MessageRequest` is transformed into a Gemini-specific JSON payload:
- **Roles**: Claw's "user" role maps to Gemini's "user", while "assistant" maps to "model".
- **Role Continuity**: Consecutive messages with the same role are merged to satisfy Gemini's strict alternating roles requirement.
- **System Instructions**: Handled via the `systemInstruction` field in the request.
- **Schema Sanitization**: Gemini is strict about JSON Schema keywords. `sanitize_schema()` recursively removes `additionalProperties`, `examples`, and `default` from tool input schemas to prevent 400 Bad Request errors.

### Tool Call Mechanics

Gemini's tool use implementation features a unique state-carrying mechanism to bridge turns:

- **ID Encoding**: Tool call IDs are formatted as `call_{idx}_{part}__@__{name}__@__{thoughtSignature}`. This allows the client to recover the tool name and `thoughtSignature` when the model refers back to a tool result in a subsequent turn.
- **Thought Signatures**: Some Gemini models require a `thoughtSignature` when responding to a `functionCall`. The client extracts this from the response and caches it (both in-memory and encoded in the ID string) to be re-sent in the next turn's `functionResponse`.

### SSE Streaming and Response Parsing

The client uses the `streamGenerateContent` endpoint with `alt=sse`.

1.  **Chunk Parsing**: Incoming data is buffered and split by newlines. Lines starting with `data: ` are parsed as JSON chunks.
2.  **Stateful Block Tracking**: `translate_response_chunk` uses a `HashSet<u32>` of `started_blocks` to track which content blocks have already emitted a `ContentBlockStart` event. This is necessary because a single SSE chunk may contain partial text for multiple parts of a model's response.
3.  **Finish Reasons**: When a candidate has a `finishReason` of `STOP`, the client ensures all started content blocks are properly closed by emitting `ContentBlockStop`.

### Retries and Quota Management

Gemini implements custom retry logic for HTTP 429 (Rate Limit Exceeded):
- It parses the `retryDelay` from the JSON error response (e.g., `"5.0s"`).
- It performs up to 3 attempts with the suggested backoff delay.
- If the suggested delay exceeds 300 seconds, it fails immediately with a `gemini_quota_exhausted` error.

### Integration with Core Prompting

The `runtime` crate recognizes Gemini models and modifies prompt construction:
- **Questioning Protocol**: Injects a strict requirement into the system prompt for the model to "Ask ONE consolidated question" and "emit a structured PLAN" before writing files.
- **Model Discovery**: Prompt builder searches for `GEMINI.md` and `GEMINI.local.md` as overrides for the standard `CLAUDE.md` instructions.

