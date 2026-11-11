# Desk notes (fund B, fictional) — we already run a solver
For intent solvers the main cost is rebalancing inventory between chains, much more than the quoting itself.
Moving USDC from Solana back to Ethereum costs us around 9 bps for each round trip in US trading hours.
Between 2 and 4 AM UTC spreads roughly double since only a handful of solvers, about three, are online.
We pre-position inventory on the destination chain ahead of the Asian session and capture the overnight spread.
Orders over 250k USD get split into smaller fills by the venue and other solvers see the flow.
Refunds increase when gas on the destination chain spikes since our quotes use stale gas numbers.
