# ProofDrop

**Provably fair puzzle contests on Solana, with the purse in escrow.** The answer is sealed on chain before the clue drops, and the prize is locked in a program account. Each guess is sealed to the solver's own wallet. When the house reveals, the program checks the reveal against the seal, keeps the earliest correct wallet and pays it. No trusted judge, and no house hand on the money.

Built by [Scrypt](https://satoshiscrypt.com) for the Colosseum **Crypto World's Fair** (Solana track).

- **Live demo:** https://proofdrop-talanham19-pngs-projects.vercel.app (devnet)
- **Program (devnet):** [`5en4PkG2UVAGsW3JfpZdfV6t7PkUh8CLpNVg37ua7T1q`](https://explorer.solana.com/address/5en4PkG2UVAGsW3JfpZdfV6t7PkUh8CLpNVg37ua7T1q?cluster=devnet)
- **Stack:** native Solana program (Rust, `solana-program` 2.2) · `@solana/web3.js` · Phantom or an in-browser wallet · static page plus one tiny faucet function. v1 runs on SPL Memo alone.

---

## The problem

Skill contests with prizes (cipher hunts, puzzle races, trivia, ARGs) run on trust. Players have to believe three things:

1. The organiser didn't change the answer after seeing who was winning.
2. Nobody saw a correct answer early and copied it.
3. The "first correct" timestamp is real.

Scrypt runs monthly cipher Sprints with cash purses. Today we enforce these rules with salted hashes in our own database, but a database is something the house controls. ProofDrop moves the promise onto a public ledger the house *can't* control.

## v2: the escrow program (`program/`)

| # | Instruction | Who | Rule enforced on chain |
|---|---|---|---|
| 0 | `CreateContest` | house | Stores `sha256(id\|salt\|ANSWER)`, the closing slot and the claim window; moves the purse into the contest PDA |
| 1 | `Submit` | solver | Only while `slot ≤ close`; one entry PDA per wallet; stores `sha256(id\|wallet\|ANSWER)` and the slot |
| 2 | `Reveal` | house | Only after close; **rejected unless it hashes to the commitment** |
| 3 | `Claim` | anyone | Recomputes the entry's hash from the revealed answer and the entry's *own* signer; keeps the earliest correct slot |
| 4 | `Payout` | anyone | After the claim window, sends the purse to the leader, or back to the house if nobody was right |

If the house never reveals, the purse can't leave escrow, so withholding a reveal can't save it money. The program does all hashing with the `sol_sha256` syscall and base58-encodes the solver key on chain, so its commitments are byte-for-byte the v1 Memo commitments: the same browser code seals guesses for both.

**Tested adversarially** (`test/escrow.test.mjs`, 17 checks, passing on a local validator and on devnet): a double entry, a late entry, an early reveal, a non-house reveal, the house changing its answer, a wrong-answer claim, a copied-hash claim (Eve re-posts Bob's sealed hash), a later correct entry trying to keep the lead, payout during the window, payout to the wrong wallet, and a double payout. All are rejected, and the earliest correct solver receives exactly the purse.

```bash
cd program && cargo-build-sbf                  # Agave 2.3 toolchain
solana program deploy --use-rpc target/deploy/proofdrop.so
node test/escrow.test.mjs                       # against solana-test-validator --bpf-program <id> proofdrop.so
node scripts/house.mjs v2-create keys/house.json PD-004 "answer" 1 216000 21600   # prints SALT, keep secret
node scripts/house.mjs v2-reveal keys/house.json PD-004 <salt> "answer"
node scripts/house.mjs v2-settle keys/house.json <housePubkey> PD-004
```

## v1: how the Memo protocol works

Every contest has its own **inbox** address. Every protocol transaction touches the inbox with a 0-lamport transfer, so the inbox's signature history is the complete, ordered public record of the contest. Each transaction carries one SPL Memo:

| Step | Signer | Memo |
|---|---|---|
| **Commit** | house | `ProofDrop:v1:COMMIT:<id>:sha256(id \| salt \| ANSWER)` |
| **Submit** | solver | `ProofDrop:v1:SUBMIT:<id>:sha256(id \| solverPubkey \| ANSWER)` |
| **Reveal** | house | `ProofDrop:v1:REVEAL:<id>:<salt>:<ANSWER>` |

**Adjudication** runs the same code in the browser and on the CLI (`lib/protocol.mjs`):

- The reveal must hash to the commit. If it doesn't, the house cheated, and the verifier says so and declares no winner.
- A submission is correct only if it equals `sha256(id | its own signer | answer)`. Re-posting someone else's correct hash from your own wallet fails, so guesses can't be copied off the chain.
- Submissions before the commit or after the reveal are ineligible.
- The winner is the earliest eligible submission by slot, with order within the inbox history breaking ties.
- Answers are normalised (case- and punctuation-insensitive) before hashing.

**Why keep v1?** It needs nothing but Solana itself: no program, no upgrade authority, about 80 lines of verifier. It proves who won. v2 goes further and *pays* who won.

## Try it (no install)

1. Open the demo and press **Create wallet**. A devnet keypair is generated in your browser, and the page's faucet (`/api/fund`) drips it 0.02 devnet SOL automatically. If the faucet is dry it falls back to Solana's airdrop, then to faucet.solana.com.
2. Solve **PD-003 · The Fair Ledger**, a puzzle written for the Crypto World's Fair, and press **Seal & submit**. Your guess is hashed in the browser with your wallet address and never leaves in plaintext.
3. On **PD-001**, press **Verify from chain** to watch the winner recomputed live from Solana.

Already use Phantom? Press **Use Phantom** (set it to Devnet under Settings → Developer Settings). Your browser wallet's key can be backed up and imported into Phantom.

## Run the house CLI

```bash
npm install
node scripts/house.mjs keygen keys/house.json          # keep keys/ out of git (.gitignore covers it)
# fund it on devnet (faucet.solana.com), then:
node scripts/house.mjs commit keys/house.json PD-003 "your answer"   # prints inbox + SALT (keep secret)
node scripts/house.mjs submit keys/solver.json PD-003 <inbox> "a guess"
node scripts/house.mjs reveal keys/house.json PD-003 <inbox> <salt> "your answer"
node scripts/house.mjs verify PD-003 <inbox> <housePubkey>
node test/protocol.test.mjs                              # adjudication tests
```

Serve the folder statically (for example `npx serve .`) and add the contest to `contests.json`.

## The faucet

`api/fund.js` is a Vercel function holding a devnet-only key in the `FAUCET_SECRET` env var (a JSON secret-key array). It funds only wallets holding less than 0.005 SOL, and allows one drip per IP or address every 30 seconds.

## One primitive, many contests

Commit, then sealed entries, then reveal and public recount. That covers more than puzzles:

| Contest | Change from puzzle mode |
|---|---|
| **Sealed-ballot voting** | Voters add their own salt (a small option set is otherwise brute-forceable) and reveal after polls close; plus an allowlist or token gate against sybils |
| **Verifiable raffles** | The house commits a seed; winner = H(seed ‖ blockhash of a slot after close) mod entries. Free entry, no purchase necessary |
| **Sealed-bid auctions** | Bids sealed with a bidder salt; highest valid reveal wins |
| **Prediction / guess-the-number** | Closest revealed guess wins; the commit fixes the true value first |
| **Bug bounties** | Prove "I found it first" by slot without disclosing the bug |

## Tests

`test/protocol.test.mjs` covers six cases:
- an honest contest with wrong, correct and late guesses
- a copied hash
- a cheating house
- forged house messages
- an open contest
- answer normalisation

## Roadmap

- **Mainnet:** embedded wallets (email sign-in) in place of browser keys, house-sponsored fees, an SPL-token purse (USDC), and a frozen upgrade authority.
- **Location-gated clues:** clue fragments unlocked only at a physical place, with the location proof committed alongside the guess. This is Scrypt's city Sprints, on chain.
- **Integration with satoshiscrypt.com's monthly Sprint**, so every real contest publishes its commitment on Solana.

## License

MIT © 2026 Scrypt LLC
