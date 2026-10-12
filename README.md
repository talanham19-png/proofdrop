# ProofDrop

**Provably fair puzzle contests on Solana, with the purse in escrow.** The answer is sealed on chain before the clue drops, and the prize is locked in a program account. Each guess is sealed to the solver's own wallet. After entries close, the program rejects any reveal that doesn't match the seal, keeps the earliest correct wallet, and pays it from escrow. **Devnet demo: every purse is play money with no cash value.**

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

## Production use: Scrypt's November Sprint

For November's cash Sprint, Scrypt will commit eight salted answer-field hashes to Solana mainnet before releasing the puzzle pack. After close it will publish the answers and salts, so anyone can verify the key stayed fixed. Scoring, identity checks and cash payouts stay off chain for now. This uses the v1 commit path, and it's the first step toward running more of the race on ProofDrop.

## v2: the escrow program (`program/`)

| # | Instruction | Who | Rule enforced on chain |
|---|---|---|---|
| 0 | `CreateContest` | house | Stores `sha256(id\|salt\|ANSWER)` and the closing slot; moves the purse into the contest PDA (the `claim_slots` field is reserved and unused since v2.1) |
| 1 | `Submit` | solver | Only while `slot ≤ close`; never from the house wallet; one entry PDA per wallet; stores `sha256(id\|wallet\|ANSWER)` and the slot |
| 2 | `Reveal` | house | Only after close; **rejected unless it hashes to the commitment** |
| 3 | `Settle` | anyone | Recomputes the entry's hash from the revealed answer and the entry's *own* signer; marks it right or wrong, once; keeps the earliest correct slot as leader |
| 4 | `Payout` | anyone | Only once **every** entry is settled: sends the purse to the earliest correct entry, or back to the house only when no correct entry exists (including a contest nobody entered) |

If the house never reveals, the purse can't leave escrow, so withholding a reveal can't save it money. The program does all hashing with the `sol_sha256` syscall and base58-encodes the solver key on chain, so its commitments are byte-for-byte the v1 Memo commitments: the same browser code seals guesses for both.

**Tested, not audited** (`test/escrow.test.mjs`: 21 checks, 14 of them rejections, each matched against its exact expected error message; run logs in `test/logs/`): a double entry, a late entry, an early reveal, a non-house reveal, the house changing its answer, the house wallet entering, settling an entry twice, a later correct entry cashing out while an earlier one is unsettled, a copied hash (Eve re-posts Bob's sealed hash, and it settles as wrong), payout to a later correct wallet, a refund to the house while a correct entry exists, and a double payout. A second contest proves the refund path: the house gets its purse back only after every entry is proven wrong. All are rejected, and the earliest correct solver receives exactly the purse.

```bash
cd program && cargo-build-sbf                  # Agave 2.3 toolchain
solana program deploy --use-rpc target/deploy/proofdrop.so
node test/escrow.test.mjs                       # against solana-test-validator --bpf-program <id> proofdrop.so
node scripts/house.mjs v2-create keys/house.json PD-005 "answer" 1 216000 0   # prints SALT, keep secret
node scripts/house.mjs v2-reveal keys/house.json PD-005 <salt> "answer"      # reveal, settle every entry, pay out
node scripts/house.mjs v2-settle keys/house.json <housePubkey> PD-005    # resume settle-and-pay if interrupted
```

### Proof on devnet (click through)

- **ESC-001, escrow paid out:** [payout transaction](https://explorer.solana.com/tx/4YHmcwXtv5DpWUbTR7dTLKpbDamiYYmpmoYYhcwUaCpdZHxomtkMD7FBbybbvXeJ2EVjBtKYbVskfAf6AZJqoV8t?cluster=devnet)
- **ESC-003, house wallet tries to enter its own contest:** [failed transaction](https://explorer.solana.com/tx/3o6K8UhoeSNPc1c7JifRJmzasmk7NippcJwTixfJc8RrRxkpxofw3ywrBPbeZ4LxAKk9MUeJdY7qsPzFmSrCSun9?cluster=devnet) ("The house wallet cannot enter its own contest")
- **ESC-003, house tries to change its answer:** [failed transaction](https://explorer.solana.com/tx/3sVkqUDnMYyWUv4gssubDiYCH2cbcTTdfWzakoKotCFJEPTKC56jcCok35Lx2pqrQ1cEksuWmT8XSWMm39cws1Xh?cluster=devnet) ("Reveal does not match the commitment")
- **ESC-003, copied entry:** [settles as wrong](https://explorer.solana.com/tx/3fMHctyC5WVjQSfrP1Lnp9wBJHzSr9ZcKmLZUQW8YEEfgsFi7soq9KBL4EVEgUVKhM3eNAR4582vxXvm2AVwy4UY?cluster=devnet); a [payout to it fails](https://explorer.solana.com/tx/5W79wANNfZMUx6FiSctmUGS6hSZFBEqQyao4cGF3iHZmuS72bn3VGvAALaLPJRAjBdXGdzXWpmuHYvoEEXDd5RnU?cluster=devnet) ("Recipient is not the winner"); the [honest winner is paid](https://explorer.solana.com/tx/2ZdM7mKb2U7wDBJNsMzcrCXHkxDgdz558gTX2eRGW6d2oMBoQ8UTjAYepTq3rx9RRmSZwaihpDEWrYpbwfbtNzSh?cluster=devnet)

Reproduce with `node scripts/proof-demo.mjs <houseKey> <winnerKey> <copierKey> <id> <answer>`.

### What you still trust

- **Devnet only.** Purses are play money with no cash value.
- **Upgrade authority.** Until it is frozen, the holder of the program's upgrade authority (Scrypt's house key) could replace the program. We freeze it before submission; the live page reads its status from chain.
- **The house knows the answer.** The program refuses entries from the house wallet, but it can't stop the house using a second wallet. That trust remains; mainnet narrows it with per-solver salts and identity-gated entry.
- **Sealed is uncopyable, not unreadable.** An entry only counts for the wallet that sent it, but someone holding a candidate answer could test it against other entries' hashes.
- **Someone has to send the transactions.** Nothing on chain runs by itself. `v2-reveal` reveals, settles every entry (wrong ones included) and pays out in one run, and it's safe to re-run. Anyone else can settle and pay out too. Payout waits until all entries are settled, so no correct entry can be skipped.
- **No reveal, no payout.** If the house never reveals, the purse stays locked forever, so withholding a reveal can't save it money.

### Deployed binary = this source

`test/logs/deployed-binary-check.txt` shows that the program on devnet is byte-for-byte this build: sha256 `a84e1185…4bf8`, followed by zero padding from the account extension. It also records the current upgrade authority.

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

1. Open the demo and press **Create wallet**. A devnet keypair is generated in your browser, and the page's faucet (`/api/fund`) sends it 0.02 devnet SOL. If the faucet is busy it falls back to Solana's airdrop, then to faucet.solana.com.
2. In **SANDBOX-01** (no purse), seal any guess and watch your entry appear on chain as "(you) · sealed".
3. In **ESC-003**, follow the proof links: the house's failed reveal (error 0x7), the house wallet's refused entry (0x10), the copied entry settling as wrong, the refused payout to the copier (0xd), and the 0.1 devnet SOL payout to the winner.
4. **PD-004 · The Fair Ledger** is the live puzzle. Its answer has been sealed since creation.

Already use Phantom? Press **Use Phantom** (set it to Devnet under Settings → Developer Settings). A browser wallet's key can be backed up and imported into Phantom.

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
