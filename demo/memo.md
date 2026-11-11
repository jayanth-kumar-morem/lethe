# Memo: where cross-chain intent solvers actually lose money (fictional demo content)
Solvers on cross-chain intent venues lose most of their margin on inventory rebalancing, not on quoting.
Rebalancing USDC from Solana back to Ethereum costs a mid-size solver roughly 9 basis points per round trip during US hours.
Quotes requested between 02:00 and 04:00 UTC clear with spreads about twice as wide because only three solvers stay online.
A solver that pre-positions inventory on the destination chain before the Asian open captures most of that overnight spread.
Large swaps above 250k USD are routinely split by the venue into smaller fills, which leaks the order to competing solvers.
Refund rates spike when destination-chain gas jumps, because quotes are priced against stale gas estimates.
A simple gas-aware deadline rule would have avoided most refunds in the sample we reviewed.
The cheapest edge for a new solver is a niche corridor such as Bitcoin to Solana stablecoins, where competition is thin.
