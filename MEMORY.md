MEMORY INDEX

Updated: 2026-04-22 19:15 UTC
Crate Map

api → rust/crates/api/src/lib.rs:1 — LLM provider abstraction layer
runtime → rust/crates/runtime/src/lib.rs:1 — conversation loop + session mgmt
cli → rust/crates/rusty-claude-cli/src/main.rs:1 — entry point, REPL, rendering
tools → rust/crates/tools/src/lib.rs:1 — built-in tool implementations
plugins → rust/crates/plugins/src/lib.rs:1 — hook/extension registry
telemetry → rust/crates/telemetry/src/lib.rs:1 — structured logging and tracing
Key Types

MessageRequest → api/src/types.rs:36 — unified LLM request type
StreamEvent → api/src/types.rs:262 — SSE event enum (Anthropic-shaped)
GoogleAiClient → api/src/providers/google_ai/mod.rs:116 — Gemini OAuth client
ConversationRuntime → runtime/src/conversation.rs:130 — main agent loop
Active Work

[Fix Rust workspace build → COMPLETE — All syntax and compile errors resolved, workspace builds successfully]
