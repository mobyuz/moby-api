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
