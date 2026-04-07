# Solana Crowdfunding Platform

A trustless crowdfunding smart contract on Solana. Creators set a goal and
deadline; donors contribute SOL into a PDA vault. Funds are only released to
the creator if the goal is met after the deadline — otherwise donors can claim
refunds.

---

## Program Architecture

```
Campaign PDA  →  seeds: ["campaign", creator_pubkey]
Vault PDA     →  seeds: ["vault",    campaign_pubkey]
```

The vault is a `SystemAccount` PDA — a plain account your program controls
via `invoke_signed`. No private key exists for it; only the program can move
funds out.

### Campaign State

| Field      | Type     | Description                        |
|------------|----------|------------------------------------|
| `creator`  | `Pubkey` | Who created the campaign           |
| `goal`     | `u64`    | Target lamports                    |
| `raised`   | `u64`    | Total lamports contributed so far  |
| `deadline` | `i64`    | Unix timestamp when campaign ends  |
| `claimed`  | `bool`   | Whether creator has withdrawn      |
| `bump`     | `u8`     | Stored PDA bump for cheap signing  |

---

## Instructions

### `create_campaign(goal, deadline)`
- Validates deadline is in the future
- Validates goal > 0
- Initialises campaign state

### `contribute(amount)`
- Validates campaign is still live (before deadline)
- Transfers SOL from donor → vault PDA via System Program CPI
- Updates `campaign.raised`

### `withdraw()`
- Requires: deadline passed, goal met, not already claimed, caller is creator
- Transfers all vault lamports → creator
- Sets `campaign.claimed = true`

### `refund(contributed)`
- Requires: deadline passed, goal NOT met
- Transfers `contributed` lamports from vault → donor
- Caller must pass in how much they originally contributed
  (production upgrade: track per-donor contributions in a separate PDA)

---

## Local Development

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Solana CLI
sh -c "$(curl -sSfL https://release.solana.com/stable/install)"

# Install Anchor
cargo install --git https://github.com/coral-xyz/anchor avm --locked
avm install 0.29.0
avm use 0.29.0

# Install Node deps
yarn install
```

### Build

```bash
anchor build
```

### Run Tests (localnet)

```bash
anchor test
```

### Deploy to Devnet

```bash
# Switch to devnet
solana config set --url devnet

# Fund your wallet
solana airdrop 2

# Build and deploy
anchor build
anchor deploy --provider.cluster devnet

# Copy the program ID output and update:
# 1. declare_id!("...") in src/lib.rs
# 2. [programs.devnet] in Anchor.toml
# Then rebuild and redeploy
anchor build && anchor deploy --provider.cluster devnet
```

---

## Security Notes

- **Double-withdraw prevention** — `campaign.claimed` is set before transfer
  and checked at instruction start.
- **`has_one = creator`** — Anchor enforces this at constraint-check time,
  before the instruction body runs.
- **Overflow checks** — `overflow-checks = true` in `Cargo.toml` release
  profile; arithmetic panics instead of wrapping.
- **Refund tracking** — the current implementation trusts the caller to pass
  their correct contribution amount. A production upgrade should store
  per-donor amounts in a `Contribution` PDA seeded by `[campaign, donor]`.

---

## Upgrade Path (Production Checklist)

- [ ] Add `Contribution` PDA to track per-donor amounts on-chain
- [ ] Add `cancel` instruction (creator can cancel before deadline if needed)
- [ ] Add minimum contribution amount
- [ ] Add campaign title/description stored off-chain (IPFS hash on-chain)
- [ ] Emit Anchor events for indexers (`emit!(CampaignCreated { ... })`)
- [ ] Add protocol fee (e.g. 1% on successful withdrawal)