#!/usr/bin/env node
// Seal a Sprint answer key on Solana with one salted commitment per field (ProofDrop v1 Memo path).
//
//   RPC_URL=https://api.mainnet-beta.solana.com \
//   node scripts/seal-sprint.mjs <houseKeyFile> <sealedKeyFile> <sealedOutFile> [--dry-run]
//
// <sealedKeyFile>  JSON in the sealed vault ONLY: {"sprint":"SPRINT-IV","fields":{"F1":"answer",...}}
// <sealedOutFile>  written to the sealed vault ONLY: per-field salt + hash + signature. Never commit it.
//
// What it prints to the terminal is the PUBLIC record only: inbox, house, and per field the id,
// hash and signature. No answer and no salt is ever printed, logged or passed on the command line.
// --dry-run computes everything and writes the sealed file but sends nothing.
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { commitHash, memoCommit, normalizeAnswer } from '../lib/protocol.mjs';
import { buildProtocolTx } from '../lib/chain.mjs';

const [keyf, keyFile, outFile, flag] = process.argv.slice(2);
if (!keyf || !keyFile || !outFile) { console.error('usage: seal-sprint.mjs <houseKey> <sealedKeyFile> <sealedOutFile> [--dry-run]'); process.exit(2); }
if (fs.existsSync(outFile)) { console.error('Refusing to overwrite an existing sealed output. Move it aside first.'); process.exit(2); }
const dry = flag === '--dry-run';
const RPC = process.env.RPC_URL || web3.clusterApiUrl('devnet');
const conn = new web3.Connection(RPC, 'confirmed');
const kp = web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(keyf, 'utf8'))));
const key = JSON.parse(fs.readFileSync(keyFile, 'utf8'));
const sprint = String(key.sprint || '').trim();
const fields = Object.entries(key.fields || {});
if (!/^[A-Z0-9-]+$/.test(sprint) || fields.length === 0) { console.error('Key file needs "sprint" (A-Z, 0-9, -) and a non-empty "fields" map.'); process.exit(2); }
for (const [f, ans] of fields) if (!normalizeAnswer(ans)) { console.error(`Field ${f} normalises to empty. Fix the key file.`); process.exit(2); }

const inbox = web3.Keypair.generate().publicKey.toBase58(); // one receive-only address carries the whole record
const sealed = { sprint, cluster: RPC, house: kp.publicKey.toBase58(), inbox, createdAt: new Date().toISOString(), fields: [] };
const pub = { sprint, cluster: RPC, house: sealed.house, inbox, fields: [] };

for (const [f, ans] of fields) {
  const id = `${sprint}-${f}`;
  const salt = crypto.randomBytes(16).toString('hex');
  const hash = await commitHash(id, salt, ans);
  let sig = null;
  if (!dry) {
    const tx = buildProtocolTx(web3, { payer: kp.publicKey, inbox, memo: memoCommit(id, hash) });
    sig = await web3.sendAndConfirmTransaction(conn, tx, [kp], { commitment: 'confirmed' });
  }
  sealed.fields.push({ id, salt, hash, sig, canonical: normalizeAnswer(ans) });
  pub.fields.push({ id, hash, sig });
  fs.writeFileSync(outFile, JSON.stringify(sealed, null, 2), { mode: 0o600 }); // written after every field, so a crash loses nothing
}
console.log(JSON.stringify({ dryRun: dry, ...pub }, null, 2));
