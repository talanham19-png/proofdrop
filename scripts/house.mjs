#!/usr/bin/env node
// ProofDrop house CLI (devnet by default).
//   node scripts/house.mjs keygen <file>
//   node scripts/house.mjs airdrop <keyfile>
//   node scripts/house.mjs commit <houseKey> <contestId> <answer>      -> prints inbox, salt (KEEP SECRET), sig
//   node scripts/house.mjs submit <solverKey> <contestId> <inbox> <guess>
//   node scripts/house.mjs reveal <houseKey> <contestId> <inbox> <salt> <answer>
//   node scripts/house.mjs verify <contestId> <inbox> <housePubkey>
// v2 (escrow program):
//   v2-create <houseKey> <id> <answer> <purseSOL> <openSlots> <reservedZero>   -> prints SALT (keep secret)
//   v2-submit <solverKey> <housePubkey> <id> <guess>
//   v2-reveal <houseKey> <id> <salt> <answer>    reveal, then settle EVERY entry and pay out
//   v2-settle <payerKey> <housePubkey> <id>      settle every unsettled entry, then pay out (resumable)
//   v2-show <housePubkey> <id>
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash, memoCommit, memoSubmit, memoReveal, adjudicate } from '../lib/protocol.mjs';
import { buildProtocolTx, fetchRecords } from '../lib/chain.mjs';
import { ixCreate, ixSubmit, ixReveal, ixPayout, settleAllTxs, loadContest, pdas, explainError } from '../lib/escrow.mjs';

const RPC = process.env.RPC_URL || web3.clusterApiUrl('devnet');
const conn = new web3.Connection(RPC, 'confirmed');
const load = f => web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(f, 'utf8'))));
const send = async (kp, inbox, memo) => {
  const tx = buildProtocolTx(web3, { payer: kp.publicKey, inbox, memo });
  return web3.sendAndConfirmTransaction(conn, tx, [kp], { commitment: 'confirmed' });
};

// Settle every unsettled entry (wrong ones included), re-read the chain, and pay out only when
// the program's own counts say every entry is settled. Safe to re-run: it resumes where it left off.
async function settleAndPay(payer, house, id) {
  let st = await loadContest(web3, conn, house, id);
  if (!st.exists) { console.log('no such contest'); process.exit(1); }
  if (!st.revealed) { console.log('not revealed yet'); process.exit(1); }
  if (st.paid) { console.log('already paid out:', st.paid === 1 ? 'winner ' + st.leader : 'refunded to house'); return; }
  for (const tx of settleAllTxs(web3, st)) {
    try { console.log('settled batch', await web3.sendAndConfirmTransaction(conn, tx, [payer], { commitment: 'confirmed' })); }
    catch (e) { console.log('settle batch failed:', explainError(e), '- re-run to resume'); process.exit(1); }
  }
  st = await loadContest(web3, conn, house, id);
  console.log(`settled ${st.settled} of ${st.entryCount}; leader ${st.leader || 'none'}`);
  if (st.settled !== st.entryCount) { console.log('not every entry is settled; re-run to resume'); process.exit(1); }
  const recipient = st.leader || st.house;
  try { console.log('paid', recipient, st.leader ? '(earliest correct entry)' : '(no correct entry; refund to house)', await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixPayout(web3, { contest: st.address, recipient })), [payer], { commitment: 'confirmed' })); }
  catch (e) { console.log('payout rejected:', explainError(e)); process.exit(1); }
}

const [cmd, ...a] = process.argv.slice(2);
if (cmd === 'keygen') {
  const kp = web3.Keypair.generate();
  fs.writeFileSync(a[0], JSON.stringify([...kp.secretKey]));
  console.log(kp.publicKey.toBase58());
} else if (cmd === 'airdrop') {
  const kp = load(a[0]);
  const sig = await conn.requestAirdrop(kp.publicKey, 1e9);
  await conn.confirmTransaction(sig, 'confirmed');
  console.log('airdropped 1 SOL to', kp.publicKey.toBase58(), sig);
} else if (cmd === 'commit') {
  const [keyf, id, answer] = a;
  const kp = load(keyf);
  const inbox = web3.Keypair.generate().publicKey.toBase58(); // receive-only address
  const salt = crypto.randomBytes(16).toString('hex');
  const h = await commitHash(id, salt, answer);
  const sig = await send(kp, inbox, memoCommit(id, h));
  console.log(JSON.stringify({ contestId: id, inbox, house: kp.publicKey.toBase58(), commitSig: sig, commitHash: h, salt }, null, 2));
} else if (cmd === 'submit') {
  const [keyf, id, inbox, guess] = a;
  const kp = load(keyf);
  const h = await submitHash(id, kp.publicKey.toBase58(), guess);
  console.log(await send(kp, inbox, memoSubmit(id, h)));
} else if (cmd === 'reveal') {
  const [keyf, id, inbox, salt, answer] = a;
  console.log(await send(load(keyf), inbox, memoReveal(id, salt, answer)));
} else if (cmd === 'verify') {
  const [id, inbox, house] = a;
  const recs = await fetchRecords(web3, conn, inbox);
  const r = await adjudicate(id, house, recs);
  console.log(JSON.stringify({
    commit: r.commit && { sig: r.commit.sig, slot: r.commit.slot, hash: r.commit.hash },
    reveal: r.reveal && { sig: r.reveal.sig, slot: r.reveal.slot, answer: r.reveal.answer },
    valid: r.valid,
    submissions: r.submissions.map(s => ({ signer: s.signer, slot: s.slot, correct: s.correct, eligible: s.eligible, sig: s.sig })),
    winner: r.winner && { signer: r.winner.signer, slot: r.winner.slot, sig: r.winner.sig },
    notes: r.notes,
  }, null, 2));
} else if (cmd === 'v2-create') {
  // v2-create <houseKey> <id> <answer> <purseSOL> <openSlots> <reservedZero>   -> prints contest + SALT (keep secret)
  const [keyf, id, answer, purseSol, openSlots, claimSlots] = a;
  const kp = load(keyf);
  const salt = crypto.randomBytes(16).toString('hex');
  const commitHex = await commitHash(id, salt, answer);
  const closeSlot = (await conn.getSlot('confirmed')) + Number(openSlots);
  const sig = await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixCreate(web3, { house: kp.publicKey, id, commitHex, closeSlot, claimSlots: Number(claimSlots), purseLamports: Math.round(Number(purseSol) * 1e9) })), [kp], { commitment: 'confirmed' });
  console.log(JSON.stringify({ program: 'v2', contestId: id, contest: pdas(web3, kp.publicKey, id).contest.toBase58(), house: kp.publicKey.toBase58(), createSig: sig, commitHash: commitHex, closeSlot, claimSlots: Number(claimSlots), salt }, null, 2));
} else if (cmd === 'v2-submit') {
  const [keyf, house, id, guess] = a; const kp = load(keyf);
  try { console.log(await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(await ixSubmit(web3, { house, id, solver: kp.publicKey, guess })), [kp], { commitment: 'confirmed' })); }
  catch (e) { console.log('rejected:', explainError(e)); process.exit(1); }
} else if (cmd === 'v2-reveal') {
  // v2-reveal <houseKey> <id> <salt> <answer> : reveal, then settle EVERY entry and pay out in the same run
  const [keyf, id, salt, answer] = a; const kp = load(keyf);
  try { console.log('revealed', await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixReveal(web3, { house: kp.publicKey, id, salt, answer })), [kp], { commitment: 'confirmed' })); }
  catch (e) { console.log('reveal rejected:', explainError(e)); process.exit(1); }
  await settleAndPay(kp, kp.publicKey.toBase58(), id);
} else if (cmd === 'v2-settle') {
  // v2-settle <payerKey> <housePubkey> <id> : settle every unsettled entry (right or wrong), then pay out
  const [keyf, house, id] = a;
  await settleAndPay(load(keyf), house, id);
} else if (cmd === 'v2-show') {
  const [house, id] = a;
  console.log(JSON.stringify(await loadContest(web3, conn, house, id), null, 2));
} else {
  console.log('usage: see header');
}
