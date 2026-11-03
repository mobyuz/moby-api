# Agent quota and service budgets

Before provider work begins, the API reserves its upper token and cost bounds
in a PostgreSQL transaction. The verified account row serializes reservations
for that account. Daily and monthly service budget rows are updated in order
with atomic cap checks. PostgreSQL time determines the UTC day. The provider
runs after the transaction commits.

The request ID identifies the reservation. A reported charge within the bound
settles it and releases the excess in a new transaction. Replaying the same
charge succeeds and preserves totals. A timeout, disconnect, restart, or an
unknown provider charge leaves the original bound counted in its UTC period.
This makes ambiguous work visible for reconciliation. Account deletion clears
the reservation's account reference and preserves service accounting. The
ledger stores amounts, dates, and IDs.

The mock counts request and terminal response UTF-8 bytes and has zero monetary
cost. A real adapter must estimate upper bounds from its configured model and
enforce the corresponding request and output limits. Costs are integer
microUSD. Both monetary caps default to zero.

`MOBY_AGENT_ENABLED` gates the route before reservation and provider work.
The circuit breaker belongs to each API process. Three consecutive failures
open it for 30 seconds, after which it admits a single probe. Disconnecting
from a probe releases its slot. PostgreSQL persists the usage ledger and
budget counters across API restarts.
