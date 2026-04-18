# Gemini Sub-agent Analysis Result

This file contains the combined output of two parallel sub-agents analyzing the Google AI / Gemini integration in Claw Code.

---

## Authentication Flow Analysis
(Produced by `auth_analyzer` sub-agent)

# Google AI Authentication Flow Summary

## 1. Loading Credentials
- **File Location**: Defaults to `$HOME/.gemini/oauth_creds.json`.
- **Override**: Can be set via `GOOGLE_OAUTH_CREDS_FILE` environment variable.
- **Loading Mechanism**: The credentials (serialized as `OAuthCreds`) are lazily loaded from the file during the first call to `get_valid_token()`.

## 2. Token Refresh
- **Expiration Check**: Tokens are considered expired if the current time is within 60 seconds of `expiry_date` or if the `expiry` string indicates an old date (e.g., starting with "2020" or "201").
- **Refresh Process**: 
    - Performs a POST request to `https://oauth2.googleapis.com/token` using `grant_type=refresh_token`.
    - Uses hardcoded `client_id` and `client_secret` if they are missing from the credentials file.
    - Updates the internal state and persists the refreshed `access_token`, `expiry_date`, and `refresh_token` back to the credentials file.

## 3. Project ID Resolution
- **Hierarchy of Resolution**:
    1. **Internal Cache**: Returns the `project_id` if already resolved and stored in memory.
    2. **Environment Variable**: Checks for `GOOGLE_CLOUD_PROJECT`.
    3. **API Discovery**: Calls the `loadCodeAssist` endpoint (`https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist`) with the OAuth token. It extracts the project ID from the `cloudaicompanionProject` field in the response.
- **Caching**: Once resolved, the `project_id` is cached in an `Arc<tokio::sync::Mutex<Option<String>>>` to avoid redundant API calls.

---

## Request Translation Analysis
(Produced by `request_analyzer` sub-agent)

# Google AI (Gemini) Request Translation Analysis

The `GoogleAiClient` in `rust/crates/api/src/providers/google_ai/mod.rs` implements a translation layer between the internal `MessageRequest` and the Google Gemini API (Vertex AI/Cloud Code variant).

## Message and Role Mapping
- **Role Translation**: Roles are mapped simply: `user` remains `user`, and all other roles (including `assistant`) are mapped to `model`. The translation layer maps messages from the internal request one-to-one to the API request, relying on higher layers to ensure appropriate message ordering and role alternation.
- **Content Parts**: Each message is converted into a Gemini "content" object containing an array of "parts":
    - **Text**: Translated to `{"text": text}`.
    - **Tool Use**: Translated to `functionCall`. It utilizes a custom ID format (`base_id__@__name__@__signature`) to preserve the `thoughtSignature` (used for reasoning models) and tool names across the stateless API boundary.
    - **Tool Result**: Translated to `functionResponse`. The tool name is recovered from the `tool_use_id` or a local `tool_call_cache`.

## System Instructions
System prompts are handled separately from the message history. If `request.system` is present, it is mapped to the `systemInstruction` field in the API payload, wrapped as a part: `{"parts": [{"text": sys}]}`.

## Tool Definitions and Schema Sanitization
Tool definitions are passed in the `tools` array as `function_declarations`. A critical `sanitize_schema` function is applied to each tool's input schema:
- **Field Removal**: Strips `additionalProperties`, `examples`, and `default` as these are often unsupported or strictly enforced by the Gemini API.
- **Type Normalization**: If `type` is provided as an array (e.g., `["string", "null"]`), it is collapsed to the first element to comply with the API's expectation of a single string type.
- **Recursion**: Recursively processes `properties` and `items` in the JSON schema.

## Payload Structure
The final payload is wrapped with metadata required by the internal Google Cloud Code endpoint:
- `model`: The target Gemini model.
- `project`: The Google Cloud Project ID (resolved via `gcloud` or environment).
- `user_prompt_id`: A fresh UUID for tracing.
- `request`: The actual translated contents, tools, and system instructions.
