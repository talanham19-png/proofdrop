# The Scrypt Collaborator Compact

One Solana program for every Scrypt collaborative project. It holds a project's treasury, keeps its roster and contribution ledger, runs the members' votes, and settles the prize. All of that is bound to the **Scrypt Collaborator Standing Offer** and its **Governance Addendum A**, ruled 27 Oct 2026. Every decision is a proposal the members vote on; no single wallet, the house's included, acts alone.

**Status:** devnet prototype. Not audited, not deployed, not compiled to SBF yet (see *To ship*). Draft and hold.

## The offer, clause by clause

| Standing Offer | Where it lives |
|---|---|
| 1% organizer's fee off the top of every settlement, to the organizer personally (not the house) | `Project.organizer` + `organizer_fee_bps` fixed at opening; never more than 1% and never zero; `ClaimOrganizer`; the sweep waits for it |
| 75% split equally among qualifiers; Scrypt LLC counts as one | `rules::settle`, `Settle`, `Claim` |
| 25% Reserve repays expenses the house fronted; the rest is the Bounty Pool | `rules::settle`; expenses can only be paid from `operating_deposits` (`Execute`) |
| Expenses beyond the Reserve come off the top of the Equal Split | `rules::settle` |
| Qualify with 3 accepted contributions; if nobody reached 3 at the win, the bar is 1 | `rules::effective_min`, applied at `Settle` |
| Bounty by documented contribution, with kinds and weights set per project | `Project.weights` (8 kinds; kind 0 = accepted contribution); `LogContribution` |
| No new member, expense, ledger entry or settlement without a vote (Addendum A) | `Propose` (ADD_MEMBER · EXPENSE · CONTRIBUTION · SETTLE) → `Vote` → `Execute` |
| Unanimous below 6 members; simple majority at 6+; ties broken by Thomas's vote, failing if he is recused | `rules::tally` |
| A passed vote is executed as passed, by anyone | `Execute` takes any signer; it can only do what passed |
| A contribution is accepted by the other members; the contributor is recused | CONTRIBUTION proposals: electorate = roster − 1, and the subject cannot vote |
| Every collaborator is paid to their own wallet, once | `Claim` (one-shot per member, to the member's own wallet) |
| Nobody can be left out of the count | `Settle` refuses unless every member account is presented exactly once |

**Per-project terms.** Each project fixes its kinds, weights, qualifying bar, vote threshold and split when it opens, and they can't be changed afterwards. Vesuvius: kind 0 *accepted contribution* = 5 points, kind 1 *GPU hour* = 1 point. A later project might weigh *illustrations delivered*, *clues tested* or *sponsor hours*. Kind labels live off-chain in the project's ledger file.

## Instructions

`CreateProject` · `Propose` · `Vote` · `Execute` · `Deposit` · `Claim` · `ClaimOrganizer` · `Sweep`. The house's only special powers: opening the project, fronting operating money (which is what expenses are repaid from), and its tie-break vote at six or more. Account lists are documented on each variant in `src/lib.rs`.

## Tests

```bash
cargo test            # rules unit tests + layout tests + two end-to-end scenarios
```

The scenarios run on the real Solana runtime (`solana-program-test`, native mode):

- **vesuvius_lane_end_to_end** — house + Ally 1, Ally 2, Ally 3:
  - an expense needs all four votes; a double vote, an outsider's vote and early execution all fail;
  - an add-member proposal dies on one "no";
  - any member executes a passed proposal; contributions go in by vote with the contributor recused (a self-vote fails); settlement is voted and refuses a missing member;
  - 50 SOL prize, 1 SOL fronted expense: house, Ally 1 and Ally 2 take 12.5 each; bounties 55/30/15%; Ally 3 is paid bounty only, being below the bar;
  - each member claims once; the sweep pays the unspent operating money only to the house wallet.
- **bar_drops_to_one_when_nobody_reached_three.**

## Not in v1 (named, not forgotten)

- **USDC.** v1 holds SOL. Prize money should sit in USDC (SPL token CPI). Do this before any real money goes in.
- **Leaving.** A member who leaves keeps their logged credit (Standing Offer item 6). v1 has no Leave instruction, so a departed member still counts in the electorate. Next version: `Leave`, which shrinks future electorates.
- **More than one prize per project.** v1 settles once. Progress Prizes plus First Letters needs per-prize settlement rounds.
- **Off-chain votes.** Submissions, publishing, scope, closing the project and breach forfeiture (Addendum A1) are voted and recorded in the ledger off chain. Next version: a generic `RECORD` proposal kind, so those votes leave an on-chain tally too.

## Before mainnet — read this

1. **Audit.** Treasury-holding programs get audited before they hold money. This one has had no audit.
2. **Legal posture.** This records *shares of a prize earned by work* and *votes on a project's spending and roster*. It mints no token, and nothing in it is transferable or sold. Keep it that way, and keep the word "equity" out of anything public. A transferable, purchasable claim on project profits starts to look like a security. Have an attorney read the Standing Offer and this README before the first real deposit.
3. **Tax.** Distributions are still payments by Scrypt LLC: W-9 before payout, 1099 at $600 and over. The chain records the payment; it doesn't file the paperwork.
4. **The paper governs.** The signed Letter of Understanding is the agreement. This program enforces it. If the two ever disagree, fix the code.

## To ship

The SBF build needs Solana's platform tools (`cargo build-sbf`), whose download is blocked from the cloud workspace. Build at the Atrium:
`sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"` → `cargo build-sbf` → `solana program deploy` to **devnet**. Then a devnet dry run with the four real wallets before anything else.
