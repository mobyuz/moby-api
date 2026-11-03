# Guest demo admission and accounting

The demo accepts a single text message and a Cloudflare Turnstile token.
The API uses a fixed HTTPS Siteverify endpoint and accepts a verified token
for the configured hostname and `demo_chat` action. Siteverify receives the
widget secret and token. The request text is used by the demo provider after
admission. [Turnstile server verification](https://developers.cloudflare.com/turnstile/get-started/server-side-validation/)

The API applies the existing PostgreSQL 15-minute limiter before Siteverify.
Its demo bucket is an HMAC of the transport peer address, grouped by /64 for
IPv6. Forwarding headers are outside the rate decision. A reverse proxy
therefore shares its transport bucket until deployment config supplies a
trusted client-address mechanism. The rate bucket expires through the existing
cleanup worker.

After bot verification, the API reserves the guest daily token allowance and
the guest daily and monthly monetary caps. PostgreSQL locks the scoped budget
rows atomically. The reservation has the `demo` meter and an empty account
reference. Settling reports the actual charge and releases the excess once.
Unknown charges retain their reservation in the original window. Migration
0007 keeps existing budget rows in the authenticated scope. Restart the API
processes together so every process uses the scoped budget key.

The provider receives one user message and empty tool and resource lists.
Only a short completed text reply is exposed. The guest demo is disabled by
default. The initial provider is the deterministic mock and costs zero.
