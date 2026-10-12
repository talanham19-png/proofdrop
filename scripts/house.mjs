#!/usr/bin/env node
// ProofDrop house CLI (devnet by default).
//   node scripts/house.mjs keygen <file>
//   node scripts/house.mjs airdrop <keyfile>
//   node scripts/house.mjs commit <houseKey> <contestId> <answer>      -> prints inbox, salt (KEEP SECRET), sig
//   node scripts/house.mjs submit <solverKey> <contestId> <inbox> <guess>
//   node scripts/house.mjs reveal <houseKey> <contestId> <inbox> <salt> <answer>
//   node scripts/house.mjs verify <contestId> <inbox> <housePubkey>
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, submitHash, memoCommit, memoSubmit, memoReveal, adjudicate } from '../lib/protocol.mjs';
import { buildProtocolTx, fetchRecords } from '../lib/chain.mjs';

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
} else {
  console.log('usage: see header');
}
