# Changelog

## Unreleased

- Added the Axum API, liveness and PostgreSQL readiness probes, and the OpenAPI contract.
- Added the local Compose stack, HTTP smoke tests, and CI configuration.
- Added the account migrations, Argon2id registration, email verification, versioned consent, and password reset through local Mailpit.
- Added SQLx query metadata and account smoke coverage for expiry, replay, and concurrent token use.
- Added signed access tokens, rotating refresh sessions, login and logout, `/v1/me`, and installation management.
- Added session revocation on refresh reuse, logout, installation revocation, and password reset.
- Added profile editing, account metadata export, password-confirmed deletion, and versioned consent management.
- Added Google OIDC, desktop PKCE handoff, explicit account linking, and Google-confirmed account deletion.
- Added the authenticated agent SSE endpoint, versioned prompt, deterministic mock, and desktop tool validation.
- Added `GET /v1/usage`, transactional UTC quota and budget reservations, an agent switch, and a provider circuit breaker.
- Added bounded WAV transcription and synthesis routes with versioned voice consent, per-modality quotas, and a local mock.
- Added a guest demo route with Turnstile verification, hashed IP limits, and a separate transactional budget.
- Added the OpenRouter LLM adapter behind `MOBY_LLM_PROVIDER`, with streamed speech, strict tool decoding, priced usage settlement, provider and model in the usage ledger, prompt v2, and an optional OpenRouter-backed guest demo. The mock stays the default.
- Accepted the optional `local_time` on agent turns. The OpenRouter adapter passes it to the model in the context message that marks every value as data, it is never logged, and prompt v2 asks the model to use it for reminder times or ask the user.
- Updated the vendored `moby-protocol` to the unreleased 0.2.0 commit `59717ba`; its `usb` module and four dependencies (`cobs`, `crc32fast`, `hmac` 0.13, `sha2` 0.11) come along unused.
