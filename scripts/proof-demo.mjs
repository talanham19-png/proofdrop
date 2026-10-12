#!/usr/bin/env node
// Leaves public, clickable proof on devnet that the program enforces its rules.
// Failed attempts are sent with preflight off, so they land on chain as failed transactions
// anyone can inspect in Solana Explorer.
//
//   node scripts/proof-demo.mjs <houseKey> <winnerKey> <copierKey> <id> <answer>
//
// 1. house creates a short contest with a small escrowed purse
// 2. winner enters the correct answer; copier re-posts the winner's sealed hash from another wallet
// 3. after close, the house tries to reveal a DIFFERENT answer  -> rejected on chain
// 4. the house reveals honestly                                -> accepted
// 5. the house wallet tries to enter its own contest             -> rejected on chain (done before close)
// 6. every entry is settled: the copier's re-posted hash settles as WRONG, the winner's as correct
// 7. a payout to the copier                                     -> rejected on chain
// 8. payout to the earliest correct wallet from escrow          -> paid
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash } from '../lib/protocol.mjs';
import { ixCreate, ixSubmit, ixSubmitRawHash, ixReveal, ixSettle, ixPayout, pdas, explainError } from '../lib/escrow.mjs';

const RPC = process.env.RPC_URL || web3.clusterApiUrl('devnet');
const conn = new web3.Connection(RPC, 'confirmed');
const load = f => web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(f, 'utf8'))));
const [hk, wk, ck, id, answer] = process.argv.slice(2);
const house = load(hk), winner = load(wk), copier = load(ck);
const ex = s => `https://explorer.solana.com/tx/${s}?cluster=devnet`;
const proof = { id, program: '5en4PkG2UVAGsW3JfpZdfV6t7PkUh8CLpNVg37ua7T1q' };

async function ok(ixx, signer) { return web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixx), [signer], { commitment: 'confirmed' }); }
async function mustFail(ixx, signer) {
  const tx = new web3.Transaction().add(ixx);
  tx.feePayer = signer.publicKey;
  const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash('confirmed');
  tx.recentBlockhash = blockhash; tx.sign(signer);
  const sig = await conn.sendRawTransaction(tx.serialize(), { skipPreflight: true });
  const res = await conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, 'confirmed');
  if (!res.value.err) throw new Error('expected failure but it succeeded: ' + sig);
  return { sig, error: explainError({ message: 'custom program error: 0x' + (res.value.err.InstructionError?.[1]?.Custom ?? 0).toString(16) }) };
}
const waitSlot = async s => { while ((await conn.getSlot('confirmed')) <= s) await new Promise(r => setTimeout(r, 1000)); };

const salt = crypto.randomBytes(16).toString('hex');
const commitHex = await commitHash(id, salt, answer);
const closeSlot = (await conn.getSlot('confirmed')) + 75;
proof.create = await ok(ixCreate(web3, { house: house.publicKey, id, commitHex, closeSlot, claimSlots: 25, purseLamports: 100_000_000 }), house);
const { contest } = pdas(web3, house.publicKey, id);
proof.contest = contest.toBase58();
proof.winnerEntry = await ok(await ixSubmit(web3, { house: house.publicKey, id, solver: winner.publicKey, guess: answer }), winner);
const winnersHash = await submitHash(id, winner.publicKey.toBase58(), answer);
proof.copierEntry = await ok(ixSubmitRawHash(web3, { house: house.publicKey, id, solver: copier.publicKey, hashHex: winnersHash }), copier);
proof.houseEnters = await mustFail(await ixSubmit(web3, { house: house.publicKey, id, solver: house.publicKey, guess: answer }), house);
console.log('house wallet tried to enter ->', proof.houseEnters.error, ex(proof.houseEnters.sig));
console.log('entries in; waiting for close at slot', closeSlot);
await waitSlot(closeSlot);
proof.houseChangesAnswer = await mustFail(ixReveal(web3, { house: house.publicKey, id, salt, answer: 'SOMETHINGELSE' }), house);
console.log('house tried to change its answer ->', proof.houseChangesAnswer.error, ex(proof.houseChangesAnswer.sig));
proof.reveal = await ok(ixReveal(web3, { house: house.publicKey, id, salt, answer }), house);
const E = k => pdas(web3, house.publicKey, id, k.publicKey).entry;
proof.copierSettled = await ok(ixSettle(web3, { contest, entry: E(copier) }), house);   // logs: "is wrong"
proof.winnerSettled = await ok(ixSettle(web3, { contest, entry: E(winner) }), house);   // logs: "is CORRECT ... new leader"
proof.payoutToCopier = await mustFail(ixPayout(web3, { contest, recipient: copier.publicKey }), copier);
console.log('payout to the copier ->', proof.payoutToCopier.error, ex(proof.payoutToCopier.sig));
proof.payout = await ok(ixPayout(web3, { contest, recipient: winner.publicKey }), house);
console.log('paid', ex(proof.payout));
console.log(JSON.stringify(proof, null, 2));
