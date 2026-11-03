# Stateless agent turns

The desktop sends the history and current tool catalog to
`POST /v1/agent/turn` with an active session. The API validates the request
before starting the response and revalidates tool proposals against the
advertised names, resource kinds and IDs. The desktop resolves those IDs,
checks local permissions and executes tools. A tool result and its matching
call appear together in the next request.

The API streams one protocol event per SSE data frame. The stream owns the
provider work, so dropping the response drops that work. The API stops on
the first terminal event. Deadline, premature provider EOF, and invalid
provider output each produce a terminal error with the API request ID.
Conversation content and tool results live in request memory.

The initial provider is a deterministic mock. It receives the [v1 system
prompt](../../src/agent/prompts/v1.txt) through the same streaming interface
that a future provider adapter will use.

The shared Rust wire types and OpenAPI schemas come from the pinned
[moby-protocol snapshot](../../vendor/moby-protocol/UPSTREAM.md). The upstream
repository needs credentials and has no tagged release. The snapshot keeps
local, Docker, and CI builds on the same contract and preserves upstream's
GPL-3.0-only license. Review distribution terms before publishing compiled
artifacts that include the protocol.
