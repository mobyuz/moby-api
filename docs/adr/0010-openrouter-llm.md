# OpenRouter LLM adapter

Date: 2026-11-04

`LlmProvider` gets its first paid implementation. `MOBY_LLM_PROVIDER` picks
`mock` (the default) or `openrouter` for the agent route, and
`MOBY_DEMO_LLM_PROVIDER` does the same for the guest demo. The adapter is one
implementation of the existing trait, so the route, the circuit breaker, the
usage ledger and the tool re-validation stay provider independent.

## Alternatives

- Call each vendor directly. This needs one adapter, key and price table per
  vendor and gives no cross-vendor fallback. The owner chose OpenRouter.
- Ask the model for one JSON object (`speech_text`, `display_text`,
  `expression_hint`, `tool`) with `response_format`. Speech would only be known
  after the closing quote of a field that must be parsed from partial JSON,
  and structured output combined with tools is not uniformly supported across
  routed providers. We rejected it.
- Model the reply as a tool such as `reply(speech, expression)`. It competes
  with the desktop's tools for the single call allowed per turn and streams
  speech as escaped argument fragments. We rejected it.

## Choice

Spoken text is the plain `content` stream. Each piece is released as a
`speech_delta` at sentence ends, and long clauses are cut at whitespace. The
agent route accepts at most 32 deltas, so the adapter coalesces model chunks
into at most 30 and flushes the rest with the terminal event. The prompt asks
for a final `<expression>idle|happy|confused</expression>` line. The adapter
withholds any possible partial tag, never sends the tag as speech, and maps the
name to `expression_hint`. A missing or unknown name becomes `idle`; system
states such as `error` or `muted` are not the model's to choose. `display_text`
stays absent because the model has no screen content to separate from speech.

Tools use native function calling. Definitions are built from the JSON Schema of
`moby_protocol::ToolRequest`, only for tools in `available_tools`. ID parameters
become enums of the matching `resources` kind. A tool whose required ID kind has
no resource is withheld, and regex `pattern` keywords with look-ahead are
dropped from the schema because providers may reject them. Decoding runs the
same strict path: the tool name and arguments are merged into the tagged
`ToolRequest` form, so unknown tools, extra fields and out-of-range values are
rejected. The adapter then applies the existing `advertised` check, and the turn
applies it again. The model never picks the `call_id`. A second call, truncated
arguments, oversized arguments or any decode failure ends the turn with one
`invalid_provider_output` error, and nothing from the call is forwarded.

Tool results are untrusted. They reach the model as JSON strings in `tool`
messages, and resource labels and locale hints travel in a system message that
calls them data. Prompt v2 states these rules, requires the user's language and
script, one to three plain spoken sentences, no claimed feelings and no invented
IDs. Prompt v1 files remain unchanged. The demo uses its own v2 prompt.

Requests send `models` (primary first), `stream`, `max_completion_tokens`,
`provider.data_collection: "deny"` and, by default, `provider.zdr: true`. With
tools they also send `tool_choice: "auto"`, `parallel_tool_calls: false` and
`provider.require_parameters: true`. The API key is a `Secret` whose `Debug`
output is redacted, and the authorization header is marked sensitive.

## Errors, retries and cancellation

| Cause | Terminal error | Hold |
| --- | --- | --- |
| 401, 402, 403, other 4xx, 429 | `provider_unavailable` | released at 0 |
| 408, transport or stream timeout | `agent_timeout` | kept |
| 5xx, transport failure, mid-stream error, early end | `provider_unavailable` | kept, or settled if usage arrived |
| bad chunk, bad tool call, empty reply, content filter | `invalid_provider_output` | settled if usage arrived |

Every error counts as a circuit-breaker failure through the existing permit.
The turn replaces provider text with fixed messages and logs only the request
ID and a fixed reason label. Response bodies of failed requests are never read.
One retry happens when no output has been sent and the request was rejected
before generation: a refused connection, 429, 502 or 503, with a wait of at
most 2 seconds. The 15 second timeout is unchanged. The stream owns the HTTP
response, so dropping the client stream drops the connection.

## Usage and budget

Prices come from `MOBY_OPENROUTER_PRICES` in microUSD per million tokens, must
be above zero, and must cover every configured model. Before the call the route
reserves input tokens, bounded by the request body bytes plus a template
allowance, and the configured output limit, at the highest configured price. The
cost bound is at least 1, so a zero budget refuses paid work. On completion the
adapter settles prompt plus completion tokens at the price of the model named in
the stream, or the highest price when that name is unknown, rounding up. Usage
above the reserved bound is not settled. Migration 0008 adds nullable `provider`
and `model` columns to `usage_reservations`. They are labeled with the
configured route at reservation and the serving model at settlement. No content
is stored. The guest demo keeps its own budget and output limit.

## Evidence

Checked on 2026-11-04 against the OpenRouter documentation:

- Streaming: <https://openrouter.ai/docs/api-reference/streaming> covers
  `: OPENROUTER PROCESSING` comments, a final chunk carrying `usage` before
  `[DONE]`, mid-stream errors as a chunk with a top-level `error` and
  `finish_reason: "error"` under HTTP 200, and cancellation by closing the
  connection for supported providers.
- Request and chunk schema: <https://openrouter.ai/docs/api-reference/chat-completion>
  lists `models`, `max_completion_tokens` (with `max_tokens` deprecated),
  `tools`, `tool_choice`, `parallel_tool_calls`, `provider`, streamed
  `delta.tool_calls` items with `index`, `id`, `type` and `function.name` and
  `arguments`, and the finish reasons `tool_calls`, `stop`, `length`,
  `content_filter` and `error`.
- Tool calling: <https://openrouter.ai/docs/guides/features/tool-calling>
  requires `tools` on every request and documents `parallel_tool_calls`.
- Usage: <https://openrouter.ai/docs/guides/administration/usage-accounting>
  says usage is always included and `usage` and `stream_options` request
  fields are deprecated. Token counts use the model's native tokenizer.
- Fallbacks: <https://openrouter.ai/docs/guides/routing/model-fallbacks> prices
  a request by the model that served it and reports it in `model`.
- Provider preferences:
  <https://openrouter.ai/docs/guides/routing/provider-selection> defines
  `data_collection`, `zdr` and `require_parameters`.
- Errors and limits: <https://openrouter.ai/docs/api-reference/errors> and
  <https://openrouter.ai/docs/api-reference/limits> define the status codes and
  `Retry-After`.

On that date OpenRouter listed `anthropic/claude-haiku-5.5` and
`openai/gpt-6-luna` at USD 0.10 per million input tokens and USD 0.50 per
million output tokens, both with tool support, per the owner. The models and
prices live in configuration, not logic.

## Limits and open points

- Tests use a local stub server. No real key or network was used. Latency,
  Uzbek quality, injection resistance and tool accuracy are unmeasured. The
  ignored live test and `live-prompts.json` exist for the owner to measure them.
- `zdr: true` and `data_collection: "deny"` are hard filters. The docs do not
  say how they combine with a `models` list, or which error appears when no
  endpoint qualifies, so a model without a matching endpoint may fail. Set
  `MOBY_OPENROUTER_ZDR=false` to drop only the ZDR filter.
- The docs do not say whether `require_parameters` or `parallel_tool_calls:
  false` shrink the set of eligible endpoints for these models.
- The protocol carries no clock or time zone, so `create_reminder` depends on
  the model producing a valid `due_at` without knowing the current time.
- Cancellation stops billing only for providers that support it.
