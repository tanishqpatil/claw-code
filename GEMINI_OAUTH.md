# Gemini Code Assist OAuth Integration

Claw Code supports native integration with Google AI and Gemini Code Assist using OAuth2 authentication. This allows users with a Google One AI Pro or Gemini Code Assist enterprise subscription to use Claw without needing a separate Anthropic API key.

## Why we built this
Claw Code originally supported Anthropic Claude and OpenAI-compatible providers. However, Gemini Code Assist is a popular enterprise-grade AI that uses Google's specialized infrastructure. Unlike other providers that use static API keys, Gemini Code Assist (especially the enterprise and Google One tiers) utilizes OAuth2 authentication. 

By adding this integration, Claw users can leverage their existing Google subscriptions directly, avoiding additional pay-per-token costs from other providers.

## How Authentication Works
Authentication for Gemini Code Assist is handled through the Google Cloud SDK (`gcloud`). Claw does not manage OAuth flows or store your credentials directly; instead, it leverages the same security context used by your terminal.

1. **OAuth Token**: Claw retrieves a short-lived OAuth2 bearer token by invoking `gcloud auth print-access-token` in a subprocess.
2. **Project Resolution**: Before sending requests, Claw calls the `loadCodeAssist` endpoint (`https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist`). This endpoint resolves the specific project ID (`cloudaicompanionProject`) associated with your account (e.g., `sincere-pen-09jrc`).
3. **Request Context**: All subsequent API calls use this project ID in the request payload and include the OAuth token in the `Authorization: Bearer <token>` header.
4. **Token Refresh**: Since OAuth tokens typically expire in one hour, Claw automatically refreshes the token via the `gcloud` subprocess whenever necessary.

## API Endpoint Structure
Claw interacts with the Internal Google Cloud Code companion APIs:

- **Base URL**: `https://cloudcode-pa.googleapis.com/v1internal`
- **Non-streaming**: `POST /:generateContent`
- **Streaming**: `POST /:streamGenerateContent?alt=sse`

### Request Format
Claw translates its internal message format into the Gemini-native JSON structure, wrapped in a proprietary envelope required by the internal endpoint:
```json
{
  "model": "gemini-2.0-flash-exp",
  "project": "sincere-pen-09jrc",
  "user_prompt_id": "550e8400-e29b-41d4-a716-446655440000",
  "request": {
    "contents": [...],
    "systemInstruction": {...},
    "tools": [...]
  }
}
```

### Response Format (Streaming)
The streaming endpoint returns standard Server-Sent Events (SSE). Each event contains a JSON chunk:
`data: {"response": {"candidates": [{"content": {"parts": [{"text": "Hello!"}]}}]}}`

## Architecture
The Gemini integration is spread across several layers in the `api` and `rusty-claude-cli` crates:

- `rust/crates/api/src/providers/google_ai/mod.rs`: The core implementation. It handles the `gcloud` subprocess calls, project ID resolution (`loadCodeAssist`), request/response translation, and the SSE streaming logic.
- `rust/crates/api/src/lib.rs`: Exports the `GoogleAiClient` and provider-specific types.
- `rust/crates/api/src/client.rs`: The `ProviderClient` dispatch layer. It dispatches calls to the `Google` variant when a `gemini-` model is detected.
- `rust/crates/api/src/providers/mod.rs`: Contains the model registry and routing logic. It detects the `Google` provider for any model starting with `gemini-` and manages aliases like `gemini` (mapping to `gemini-2.5-pro`) and `gemini-flash`.
- `rust/crates/rusty-claude-cli/src/main.rs`: The main CLI entry point. It uses `detect_provider_kind` to select the Google backend and initializes the `AnthropicRuntimeClient` (which, despite its name, acts as a general provider wrapper).

## Streaming Implementation & Bug Fixes
The streaming implementation for Gemini required careful tuning of the SSE parsing and rendering pipeline. During development, four critical bugs were identified and fixed:

1. **Missing Line Extraction**: A bug in the SSE parser loop caused extracted lines to be ignored. The loop correctly identified newlines but didn't pass the actual line content to the JSON parser, causing all response chunks to be silently dropped.
2. **Eager Streaming ( find_stream_safe_boundary )**: The `MarkdownStreamState` was too conservative, only yielding text when a blank line or paragraph break was encountered. This caused long delays in output. We modified the boundary logic to yield output on every newline when not inside a code block.
3. **Terminal Cursor Preservation**: The `TerminalRenderer` was stripping trailing newlines from rendered markdown. This left the cursor at the end of the text line. When the CLI's "Done" spinner finished, its `Clear(CurrentLine)` command would wipe out the entire response. We updated the state manager to preserve these trailing newlines.
4. **Final Flush Newline**: Even after Fix 3, the very last chunk of a message (the `MessageStop` event) often lacked a trailing newline because the renderer trims the final output. We added an explicit `writeln!(out)` in the `MessageStop` handler in `consume_stream` to ensure the cursor moves to a new line before the spinner clears it.

## Request/Response Translation
Claw acts as a bridge between Claude's API format and Gemini's:

- **Text**: Gemini's `parts[].text` chunks are mapped to `ContentBlockDelta(TextDelta)`.
- **Tool Calls**: Gemini's `functionCall` objects are translated into Claude-style `ToolUse` starts and `InputJsonDelta` events.
- **Thinking Tokens**: Gemini 2.0/2.5 "thinking" tokens (provided via `thoughtSignature` in the API) are parsed and mapped to the thinking block format.
- **Stop Reasons**: Gemini's `finishReason: STOP` is translated to `ContentBlockStop` and `MessageStop`.

## Tool Call Translation
Claw's tool definitions (which follow the Claude JSON Schema format) are translated to Gemini's `function_declarations`.
- The `input_schema` is sanitized to remove fields Gemini does not support (like `additionalProperties` or `examples`).
- Tool results from Claw are mapped back to Gemini's `functionResponse` parts in subsequent requests.

## Limitations and Known Issues
- **Subprocess Dependency**: Authentication requires the `gcloud` CLI to be installed and authenticated (`gcloud auth login`).
- **Grounding**: Gemini-specific features like Google Search grounding or the built-in code execution tool are not currently surfaced.
- **Thinking Tokens**: While `thoughtSignature` is parsed, the internal thinking text is not yet rendered to the user in the TUI.
- **Internal API**: This integration uses Google's internal Cloud Code companion API, which has a slightly different response envelope (`{"response": {...}}`) than the public Vertex AI or Google AI SDKs.
