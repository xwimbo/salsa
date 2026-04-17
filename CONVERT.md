# Chat Completions Conversion Plan

This document is a step-by-step plan for converting Salsa's agent runtime from the current Responses-style integration to a Chat Completions-style integration.

The goal is to preserve the existing UX and phased agent loop while swapping the provider transport and message/tool-call shape underneath it.

The current implementation is centered around these files:
- `src/api/codex.rs`
- `src/api/mod.rs`
- `src/agent/mod.rs`
- `src/agent/provider.rs`
- `src/main.rs`
- `src/tools/mod.rs`
- `src/app/mod.rs`
- `src/models/mod.rs`

The current live behavior to preserve is:
- streaming deltas into the UI
- phased execution across `Plan`, `Explore`, `Act`, `Verify`, `Respond`
- tool execution and tool-result feedback loops
- background specialist jobs
- cron-triggered background work
- workspace sandboxing and project switching
- current worker/event model

Because the built-in Codex-backed provider is tied to the current endpoint shape, this plan assumes a temporary stub or local provider will be used during the migration.

---

## Recommended temporary provider during migration

### Preferred default for now
- Keep a stub provider available and make it the safe default fallback during the migration.
- The existing `EchoProvider` is too limited for realistic phased-loop testing because it does not produce tool calls.
- Add a richer temporary provider that can simulate tool-calling behavior deterministically for UI and loop validation.

### Best practical local-model option
If you want a small local model for testing the chat-completions path before a real hosted provider is chosen, the safest observed direction is:
- run a local OpenAI-compatible server such as `llama.cpp` server mode or Ollama with an OpenAI-compatible bridge
- use a small instruct model in the 7B–8B class for tool-call and planning experiments

Reasonable candidates for experimentation:
- `Qwen2.5-Coder:7B` or similar coder-tuned small model
- `Llama 3.1 8B Instruct` if you want broad general chat behavior

Important caveat:
- small local models are often inconsistent at structured tool calling, especially across multi-step phased loops
- therefore the migration plan below treats a deterministic stub provider as the required baseline, and any local model as optional validation on top

---

## Success criteria

The migration is complete when all of the following are true:
- [ ] No production code depends on the Responses-style request body shape (`instructions`, `input`, `output_text`, `input_text`, etc.)
- [ ] Provider messages are serialized into Chat Completions-compatible `messages`
- [ ] Tool schemas are emitted in Chat Completions-compatible `tools` format
- [ ] Tool calls are parsed from Chat Completions streaming events or final message payloads
- [ ] The specialist loop still supports multiple tool iterations per phase
- [ ] The UI still receives `WorkerEvent::Delta`, `ToolCalls`, `PhaseChange`, `StepUpdate`, `BoardUpdate`, and job events in the same way it does now
- [ ] A temporary provider exists that supports realistic testing while the Codex-backed provider is unavailable
- [ ] `cargo test` and `cargo build` pass after each major milestone

---

## Implementation strategy

Use a two-track strategy:
1. First isolate the provider transport layer behind cleaner abstractions without changing UX.
2. Then implement Chat Completions request/response handling.

Do not try to rewrite the entire loop in one pass. The current loop in `src/agent/provider.rs` is already complex and stateful. Smaller models will succeed more reliably if the conversion is broken into narrow checkpoints.

---

# Step-by-step plan

## 1. Freeze and document the current transport assumptions
- [x] Audit all code that assumes Responses API semantics.
- [x] Record each assumption in code comments or follow-up notes during implementation.

### Why
Right now the code is not merely “using an endpoint”; it is built around the Responses payload model.

### Observed assumptions to replace
- `src/agent/mod.rs:34-64` serializes `ProviderMessage::as_json()` into Responses-style content items such as `input_text` and `output_text`
- `src/agent/provider.rs:341-352` builds direct-chat requests using `instructions` plus `input`
- `src/agent/provider.rs:508-518` builds specialist-loop requests using `instructions`, `input`, and Responses tool settings
- `src/api/codex.rs:42` posts to `https://chatgpt.com/backend-api/codex/responses`
- `src/api/codex.rs:127-233` parses Responses/SSE event shapes such as `response.output_text.delta`
- `src/api/codex.rs:127-165` finds tool calls by recursively scanning for `function_call` objects in Responses payloads

### Deliverable
A short internal checklist in implementation notes or commit messages enumerating every place where Responses semantics are assumed.

### Step 1 implementation notes
- `src/agent/mod.rs:36-69` serializes `ProviderMessage` into Responses-only content arrays using `input_text`, `output_text`, `input_image`, and `input_file` item types.
- `src/agent/provider.rs:372-383` sends direct-chat requests with top-level `instructions`, `input`, and `store` fields instead of Chat Completions `messages`.
- `src/agent/provider.rs:564-574` sends specialist-turn requests with top-level `instructions`, `input`, `tools`, and `tool_choice` fields shaped for the legacy Responses transport.
- `src/api/codex.rs:47` hardcodes the legacy endpoint `https://chatgpt.com/backend-api/codex/responses`.
- `src/api/codex.rs:87-246` parses legacy SSE event names such as `response.output_text.delta`, reconstructs final text from `output_text`, and recursively scans payloads for `function_call` objects.
- `src/tools/mod.rs:379-607` emits flattened function tool specs like `{ "type": "function", "name": ..., "parameters": ... }`, which differ from nested Chat Completions tool schemas.
- `src/main.rs:104-111` hard-switches runtime provider setup to the legacy `CodexProvider` when auth loads, leaving no transport-selection layer for migration providers.

---

## 2. Introduce a provider-transport abstraction before changing behavior
- [ ] Create a new abstraction layer in `src/api/` that separates request building, HTTP transport, and stream parsing from the agent loop.
- [ ] Keep the old implementation working initially.

### Why
`src/agent/provider.rs` currently knows too much about the transport payload shape. That will make the conversion brittle.

### Suggested sub-steps
1. Add a transport-neutral result type for one model turn, for example:
   - streamed text delta accumulation
   - parsed tool calls
   - optional finish reason
   - optional raw payload for diagnostics
2. Move request-shape concerns out of `src/agent/provider.rs` and into `src/api/`.
3. Make the provider loop ask for “model turn execution” instead of hand-building endpoint JSON inline.

### Files likely touched
- `src/api/mod.rs`
- `src/api/codex.rs` or its replacement
- `src/agent/provider.rs`

### Checkpoint
- [ ] The loop still works with the old transport after this refactor.

---

## 3. Replace Responses-specific message serialization with chat-completions serialization
- [x] Introduce Chat Completions message serialization alongside the current `ProviderMessage::as_json()`.
- [x] Do not delete the old serializer until the new path is proven.

### Why
Chat Completions expects a `messages` array with role-tagged objects, not the current Responses `input` array with typed content entries.

### Current code that must change
- `src/agent/mod.rs:34-64`

### Required design decisions
1. Decide how to map `Role` values:
   - `User` -> `user`
   - `Assistant` -> `assistant`
   - `System` -> `system`
   - `ToolResult` likely becomes `tool` or synthetic assistant/context feedback depending on chosen loop design
2. Decide how to represent media attachments for a Chat Completions-compatible provider.
   - If the temporary provider cannot support attachments, explicitly gate or stub attachment support.
3. Decide how to encode continuation frames.
   - They are currently appended as synthetic `System` messages; this is likely still fine.

### Recommended implementation
Add a new serializer method such as:
- `ProviderMessage::as_chat_completion_message()`

Do not overload the existing method with conditional behavior at first. Keep the formats separate while migrating.

### Checkpoint
- [x] The code can build a valid Chat Completions `messages` array from the current in-memory conversation model.

### Step 3 implementation notes
- `src/agent/mod.rs` now keeps the legacy `ProviderMessage::as_json()` serializer and adds `ProviderMessage::as_chat_completion_message()` plus attachment conversion for Chat Completions-style content parts.
- `Role::ToolResult` currently maps to a synthetic `system` message in the Chat Completions serializer so the existing continuation-frame/tool-feedback design stays intact until step 9.
- `src/agent/provider.rs` now builds a transport-ready Chat Completions `messages` preview via `build_chat_completion_messages(...)`, including the current instruction string as a leading `system` message.
- The active transport still sends the legacy Responses request body; the new Chat Completions serialization is kept alongside it as the migration target until request builders switch over in later steps.

---

## 4. Define a transport-neutral tool-call model and normalize all providers to it
- [x] Confirm that `ToolCall { id, name, args }` remains the canonical internal representation.
- [x] Ensure every provider path converts external tool-call payloads into that struct.

### Why
The internal loop already works well if it receives `Vec<ToolCall>`. Keep that stable.

### Current advantage
- `src/api/codex.rs:15-20` already defines a minimal `ToolCall` structure
- `src/agent/provider.rs` already consumes it cleanly after parsing

### Tasks
1. Move `ToolCall` into a transport-neutral location if needed.
2. Add comments clarifying that all providers must normalize into this internal shape.
3. Keep `dedupe_tool_calls()` unchanged initially if possible.

### Checkpoint
- [x] Tool-call handling in `run_specialist_loop()` remains unchanged after transport refactor.

### Step 4 implementation notes
- `src/api/mod.rs` is now the canonical home of `ToolCall { id, name, args }`, and `ModelTurnTransport` explicitly requires providers to normalize backend payloads into that shape.
- `src/api/codex.rs` now constructs normalized tool calls through `ToolCall::new(...)` while parsing legacy Responses payloads.
- `src/agent/provider.rs` continues to consume `Vec<ToolCall>` unchanged in `dedupe_tool_calls(...)` and the specialist loop, preserving loop behavior while transport implementations normalize upstream.

---

## 5. Convert tool schema emission from current Responses-oriented specs to Chat Completions-compatible specs
- [x] Audit every tool schema emitted from `src/tools/mod.rs` and related tool files.
- [x] Verify they match the `tools: [{ type: "function", function: { name, description, parameters } }]` shape expected by the chosen Chat Completions-compatible backend.

### Why
The current tool spec builder looks close to function-calling already, but compatibility is not guaranteed because different endpoints are stricter about nesting.

### Current risk
The current code emits specs like:
- `{"type":"function","name":"fs_read",...}`

Many chat-completions implementations instead expect:
- `{"type":"function","function":{"name":"fs_read","description":"...","parameters":{...}}}`

### Tasks
1. Inspect every `spec()` function in `src/tools/`.
2. Decide whether to:
   - update all existing spec builders directly, or
   - add a conversion adapter that wraps the current internal schema into chat-completions format
3. Prefer an adapter first if you want to minimize disruption.

### Checkpoint
- [x] The transport layer can request tools in a schema format accepted by the new backend without changing tool executor behavior.

### Step 5 implementation notes
- All current tool schema builders still emit the legacy flattened shape, so `src/tools/mod.rs` now wraps every emitted spec through `wrap_chat_completion_tool_spec(...)`.
- The adapter converts `{ type, name, description, parameters }` into Chat Completions-compatible `{ type: "function", function: { name, description, parameters } }` without changing tool executors or individual spec builders.
- `src/agent/provider.rs` now includes the wrapped tool schemas in the Chat Completions preview payload alongside the new `messages` preview.

---

## 6. Design the Chat Completions request builder for both direct chat and phased specialist turns
- [ ] Create explicit request builders for the two main request modes.

### Why
There are two different behaviors today:
1. direct chat with no tools
2. phased specialist turns with optional tools

### Suggested builders
- `build_direct_chat_completion_request(...)`
- `build_specialist_chat_completion_request(...)`

### What each builder must include
#### Direct path
- model
- messages
- stream
- no tools

#### Specialist path
- model
- messages
- stream
- tools
- tool choice policy
- any provider-specific flags that are supported by the chosen backend

### Important migration note
The current code relies heavily on `instructions` separate from `input`. Chat Completions usually requires you to fold those instructions into one or more `system` messages.

### Required adaptation
Every current instruction string builder must be inserted into the `messages` array, probably as a leading `system` message.

### Current code affected
- `src/agent/provider.rs:332-359`
- `src/agent/provider.rs:490-518`
- `src/agent/provider.rs:723-809`

### Checkpoint
- [ ] The loop can produce transport-ready Chat Completions JSON bodies for both direct and specialist turns.

---

## 7. Replace the current SSE parser with a Chat Completions stream parser
- [ ] Implement a new streaming parser for Chat Completions responses.
- [ ] Keep it isolated from the specialist loop.

### Why
This is the most endpoint-specific part of the system.

### Current parser behavior to preserve
- accumulate assistant text
- emit `WorkerEvent::Delta` as text arrives
- extract tool calls
- surface API errors cleanly
- stop on stream completion markers

### Current code affected
- `src/api/codex.rs:60-123`
- `src/api/codex.rs:167-233`

### Required parser behavior for the new endpoint
The new parser must handle streaming chunks that may split tool-call data across multiple events. Smaller models often miss this detail.

### Sub-steps
1. Add a chunk accumulator for assistant text.
2. Add a chunk accumulator keyed by tool-call index or id for function arguments streamed incrementally.
3. Only emit finalized `ToolCall` values once the stream or choice is complete enough to reconstruct them.
4. Preserve incremental `WorkerEvent::Delta` text emission for the UI.
5. Translate backend error payloads into `anyhow!` errors with body excerpts.

### Special warning
Do not assume tool-call arguments arrive as a complete JSON string in a single event. Plan for partial fragments.

### Checkpoint
- [ ] A single model turn can stream text and/or tool calls through the new parser into the existing worker event system.

---

## 8. Update the specialist loop to use Chat Completions messages instead of Responses `instructions` + `input`
- [ ] Refactor `run_specialist_loop()` so it no longer constructs Responses-style bodies inline.

### Why
Once the request builder and parser exist, the loop should become transport-agnostic.

### What should remain unchanged
- phase sequencing
- board updates
- tool execution
- continuation frames
- event emission shape
- job handling
- budgets and iteration limits

### What should change
- request construction should call the new request builder
- returned parsed chunks should come from the new transport client
- instruction strings should become system messages instead of top-level `instructions`

### Suggested refactor shape
Replace this pattern:
- build JSON body in `run_specialist_loop()`
- call `client.request(...)`

With this pattern:
- build a transport-neutral request object or chat-completions body through helper
- call `client.chat_completion_turn(...)`

### Checkpoint
- [ ] The specialist loop source no longer contains Responses-specific body fields.

---

## 9. Decide how tool results should be fed back into the conversation under Chat Completions
- [ ] Explicitly redesign the follow-up conversation message shape after tool execution.

### Why
This is the biggest semantic difference after streaming.

### Current behavior
After a tool runs, the loop does not append a classic Chat Completions `tool` role message with `tool_call_id`. Instead it mostly stores summaries and continuation frames, and only appends `Role::ToolResult` messages for media attachments.

### Decision required
Choose one of these patterns and implement it consistently:

#### Option A: Native tool-message feedback
- append assistant tool-call message
- append tool result messages keyed by tool call id
- let the next model request continue naturally

#### Option B: Keep current continuation-frame design, but encode tool outcomes as synthetic system messages
- easier migration from the current loop
- less idiomatic for chat completions tool calling
- may reduce compatibility with providers that expect explicit tool-result roles

### Recommendation
Prefer Option A for long-term correctness.

### Concrete tasks
1. Introduce internal support for associating tool results with tool-call ids.
2. Append explicit tool result messages after each tool execution.
3. Keep continuation frames for summarized history compression, but do not rely on them as the only tool feedback path.
4. Preserve attachment handling separately.

### Checkpoint
- [ ] The loop feeds tool outputs back to the model in a Chat Completions-compatible way.

---

## 10. Redesign `ProviderMessage` and/or add new types only if necessary
- [ ] Decide whether the current `ProviderMessage` abstraction is still sufficient.

### Why
The current type was designed around Responses API content modeling plus attachments.

### Decision criteria
Keep `ProviderMessage` if it can cleanly support:
- normal chat-completions messages
- tool result messages
- system instruction messages
- future provider variations

If it cannot, introduce a second internal transport-layer message enum rather than overloading one type too much.

### Recommendation
Do not force `ProviderMessage` to become a perfect mirror of every transport. It is acceptable to introduce a dedicated transport-layer request message representation if that simplifies the migration.

### Checkpoint
- [ ] Message modeling is clear enough that smaller models can work on transport code without breaking app logic.

---

## 11. Build a real temporary stub provider for migration testing
- [ ] Add a richer temporary provider instead of relying only on `EchoProvider`.

### Why
The phased loop needs to exercise:
- streaming text
- tool call emission
- multiple tool iterations
- final response synthesis

The current echo provider only validates text streaming.

### Recommended provider behavior
Create something like `StubCompletionsProvider` that:
1. inspects the last user message
2. emits predictable tool calls for trigger phrases such as:
   - `list` -> `fs_list`
   - `read` -> `fs_read`
   - `write` -> `fs_write`
   - `test` -> `sh_run`
3. can optionally return plain assistant text for non-tool prompts
4. supports streaming deltas into `WorkerEvent::Delta`
5. can simulate a tool-call-followed-by-final-answer sequence

### Why this is better than a tiny local model during migration
- deterministic
- no external dependency
- easier for smaller models to validate
- suitable for CI later if tests are added

### Checkpoint
- [ ] The app can test the full phased loop without the Codex provider.

---

## 12. Add an optional OpenAI-compatible local provider for manual testing
- [ ] Add a separate provider implementation that talks to a configurable local OpenAI-compatible endpoint.

### Why
This gives you realistic manual testing without depending on Codex-specific auth or backend behavior.

### Suggested design
Create something like:
- `LocalChatCompletionsProvider`

### Configuration it should read
- base URL
- API key or dummy token
- model name
- request timeout

### Expected use
- manual validation only at first
- not required for the core migration

### Good first target
A provider that can talk to an OpenAI-compatible local server, then point it at:
- Ollama bridge
- llama.cpp server
- another local compatible runtime

### Checkpoint
- [ ] Manual test path exists for chat-completions behavior outside the stub provider.

---

## 13. Refactor `main.rs` provider selection so migration providers are easy to switch
- [ ] Make provider selection explicit and configurable.

### Why
Right now `src/main.rs:99-105` hard-switches between `CodexProvider` and `EchoProvider` depending on auth loading. That is too rigid for migration.

### Tasks
1. Introduce a small provider-selection layer.
2. Support at least:
   - stub provider
   - local chat-completions provider
   - legacy codex provider while it still exists
3. Default safely to the stub if the configured provider cannot initialize.
4. Reflect the selected provider label in the existing top-bar status text.

### Checkpoint
- [ ] You can boot the app against the stub provider without touching source code each time.

---

## 14. Keep cron and background jobs working by converting them last, not first
- [ ] Delay cron/provider edge-case work until the main interactive loop is converted.

### Why
`cron_scheduler.rs` depends on the same provider machinery but is not the best first validation target.

### Tasks
1. After interactive turns work, re-test background specialist jobs.
2. Then re-test cron-triggered jobs.
3. Ensure the converted provider path still emits:
   - `JobStarted`
   - `JobUpdated`
   - `JobMessage`
4. Confirm that background jobs still use the same `run_specialist_loop()` entry point.

### Checkpoint
- [ ] Background and cron jobs work with the new transport path.

---

## 15. Add conversion-focused tests before deleting the old transport
- [ ] Add tests around request building and stream parsing.

### Why
There are currently no checked-in tests, and this migration is highly protocol-sensitive.

### Minimum useful tests
1. Message serialization tests
   - system/user/assistant/tool message conversion to chat completions
2. Tool schema conversion tests
   - current tool spec -> chat-completions-compatible shape
3. Stream parser tests
   - plain streamed text
   - streamed tool call with fragmented arguments
   - error payload handling
4. Loop integration tests around the stub provider
   - text-only turn
   - tool-using turn
   - multi-phase turn with tool usage and final response

### Checkpoint
- [ ] The new transport has tests before the old path is removed.

---

## 16. Delete or quarantine the legacy Responses transport only after parity is proven
- [ ] Remove or isolate the old transport path once the new one passes compile and behavior checks.

### Why
Premature deletion will make debugging much harder.

### Tasks
1. Keep the legacy transport behind a feature flag or separate module during transition.
2. Once the chat-completions path is stable, remove:
   - Responses-only request builders
   - Responses-only SSE parsing branches
   - unused serialization helpers such as `input_text`/`output_text` forms
3. Update docs:
   - `AGENTS.md`
   - `README.md`
   - any code comments that mention the old endpoint

### Checkpoint
- [ ] The codebase no longer depends on the old endpoint shape.

---

# Execution order for smaller models

If this work is split across smaller models, use this order:

## Batch A: transport isolation
- [ ] Step 1
- [ ] Step 2
- [ ] Step 4

## Batch B: message and tool schema migration
- [ ] Step 3
- [ ] Step 5
- [ ] Step 10

## Batch C: request building and parsing
- [ ] Step 6
- [ ] Step 7

## Batch D: loop conversion
- [ ] Step 8
- [ ] Step 9

## Batch E: temporary providers
- [ ] Step 11
- [ ] Step 12
- [ ] Step 13

## Batch F: background systems and cleanup
- [ ] Step 14
- [ ] Step 15
- [ ] Step 16

---

# File-by-file migration checklist

## `src/api/mod.rs`
- [ ] Add new chat-completions transport module(s)
- [ ] Re-export only the stable abstractions you want providers to use

## `src/api/codex.rs`
- [ ] Either rename to reflect legacy transport or replace with a chat-completions client
- [ ] Remove hard dependency on `/backend-api/codex/responses`
- [ ] Replace Responses SSE parsing logic

## `src/agent/mod.rs`
- [ ] Add or replace message serialization helpers
- [ ] Stop encoding messages in Responses content-item format once migration is complete

## `src/agent/provider.rs`
- [ ] Stop building endpoint-specific JSON inline
- [ ] Route through new request builders/client helpers
- [ ] Preserve loop, event, and job semantics
- [ ] Add support for explicit tool-result feedback if adopting native tool-message flow

## `src/main.rs`
- [ ] Replace hardcoded provider fallback logic with configurable selection
- [ ] Support stub provider and optional local provider

## `src/tools/mod.rs` and tool modules
- [ ] Confirm schema compatibility with chat completions function tools
- [ ] Adjust schema wrapping if necessary

## `README.md`
- [ ] Update provider/auth notes after migration direction is chosen

## `AGENTS.md`
- [ ] Update repository guidance after the transport conversion lands

---

# Risks and pitfalls

## 1. Streaming tool-call arguments may arrive fragmented
Do not assume a single chunk contains valid final JSON arguments.

## 2. The current continuation-frame design may fight native tool-message flow
If the model performs poorly after migration, this is one of the first places to inspect.

## 3. Small local models may appear “broken” when the real issue is tool-call discipline
Use the deterministic stub provider first so transport bugs and model-quality issues do not get confused.

## 4. Attachments/media support may not map cleanly to the temporary provider
Gate it explicitly during migration instead of letting it fail ambiguously.

## 5. Top-level `instructions` are going away conceptually
Anything currently depending on instruction priority must be preserved by placing that content into leading `system` messages consistently.

## 6. Worker/UI compatibility must remain stable
Do not redesign `WorkerEvent` unless absolutely necessary. The UI already depends on the current event model.

---

# Minimum viable migration path

If you want the shortest path to a working system, do this:
- [ ] Add a deterministic stub chat-completions provider
- [ ] Add a chat-completions message serializer
- [ ] Add a chat-completions request builder
- [ ] Add a chat-completions stream parser
- [ ] Convert `run_specialist_loop()` to use it
- [ ] Keep cron/background jobs using the same loop
- [ ] Only after that, decide what real hosted or local provider replaces Codex

This path preserves the most existing logic while replacing only the protocol-specific layers first.

---

# Final recommended first implementation ticket

Start with this concrete ticket:

- [ ] "Introduce a transport-neutral model turn client, add a deterministic stub provider that supports streaming text and synthetic tool calls, and refactor `run_specialist_loop()` so request construction/parsing are no longer hardcoded to the Responses API."

That ticket is the best first checkpoint because it reduces migration risk without requiring the final real provider decision yet.
