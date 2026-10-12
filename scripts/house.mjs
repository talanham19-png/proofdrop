#!/usr/bin/env node
// ProofDrop house CLI (devnet by default).
//   node scripts/house.mjs keygen <file>
//   node scripts/house.mjs airdrop <keyfile>
//   node scripts/house.mjs commit <houseKey> <contestId> <answer>      -> prints inbox, salt (KEEP SECRET), sig
//   node scripts/house.mjs submit <solverKey> <contestId> <inbox> <guess>
//   node scripts/house.mjs reveal <houseKey> <contestId> <inbox> <salt> <answer>
//   node scripts/house.mjs verify <contestId> <inbox> <housePubkey>
// v2 (escrow program):
//   v2-create <houseKey> <id> <answer> <purseSOL> <openSlots> <claimSlots>   -> prints SALT (keep secret)
//   v2-submit <solverKey> <housePubkey> <id> <guess>
//   v2-reveal <houseKey> <id> <salt> <answer>
//   v2-settle <payerKey> <housePubkey> <id>      claim correct entries, then pay out after the window
//   v2-show <housePubkey> <id>
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash, memoCommit, memoSubmit, memoReveal, adjudicate } from '../lib/protocol.mjs';
import { buildProtocolTx, fetchRecords } from '../lib/chain.mjs';
import { ixCreate, ixSubmit, ixReveal, ixClaim, ixPayout, loadContest, pdas, explainError } from '../lib/escrow.mjs';

const RPC = process.env.RPC_URL || web3.clusterApiUrl('devnet');
const conn = new web3.Connection(RPC, 'confirmed');
const load = f => web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(f, 'utf8'))));
const send = async (kp, inbox, memo) => {
  const tx = buildProtocolTx(web3, { payer: kp.publicKey, inbox, memo });
  return web3.sendAndConfirmTransaction(conn, tx, [kp], { commitment: 'confirmed' });
};

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
  // v2-create <houseKey> <id> <answer> <purseSOL> <openSlots> <claimSlots>   -> prints contest + SALT (keep secret)
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
  catch (e) { console.log('rejected:', explainError(e)); }
} else if (cmd === 'v2-reveal') {
  const [keyf, id, salt, answer] = a; const kp = load(keyf);
  try { console.log(await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixReveal(web3, { house: kp.publicKey, id, salt, answer })), [kp], { commitment: 'confirmed' })); }
  catch (e) { console.log('rejected:', explainError(e)); }
} else if (cmd === 'v2-settle') {
  // v2-settle <payerKey> <house> <id> : claims every correct entry (earliest wins), then pays out once the window closes
  const [keyf, house, id] = a; const kp = load(keyf);
  const st = await loadContest(web3, conn, house, id);
  for (const e of st.entries.filter(e => e.correct)) {
    try { await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixClaim(web3, { contest: st.address, entry: e.address })), [kp], { commitment: 'confirmed' }); console.log('claimed', e.solver); }
    catch (err) { console.log('claim', e.solver, '->', explainError(err)); }
  }
  const after = await loadContest(web3, conn, house, id);
  const end = after.revealSlot + after.claimSlots;
  while ((await conn.getSlot('confirmed')) <= end) await new Promise(r => setTimeout(r, 1000));
  const recipient = after.leader || after.house;
  try { console.log('paid', recipient, await web3.sendAndConfirmTransaction(conn, new web3.Transaction().add(ixPayout(web3, { contest: st.address, recipient })), [kp], { commitment: 'confirmed' })); }
  catch (err) { console.log('payout ->', explainError(err)); }
} else if (cmd === 'v2-show') {
  const [house, id] = a;
  console.log(JSON.stringify(await loadContest(web3, conn, house, id), null, 2));
} else {
  console.log('usage: see header');
}
