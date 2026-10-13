#!/usr/bin/env node
// Freeze the ProofDrop program for good: BPF Upgradeable Loader `SetAuthority` with NO new authority.
// This cannot be undone. It was run once, on 13 Oct 2026 UTC; the result is in test/logs/freeze-check.txt.
//   node scripts/freeze.mjs <authorityKey.json>          verify the deployed binary, then simulate only
//   node scripts/freeze.mjs <authorityKey.json> --send   send, confirm, read back
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { PROGRAM_ID } from '../lib/escrow.mjs';

const conn = new web3.Connection(process.env.RPC_URL || web3.clusterApiUrl('devnet'), 'confirmed');
const LOADER = new web3.PublicKey('BPFLoaderUpgradeab1e11111111111111111111111');
const BUILD_LEN = 112096;
const BUILD = fs.readFileSync(new URL('../test/logs/build-hash.txt', import.meta.url), 'utf8').trim().split(/\s+/)[0];
const [programData] = web3.PublicKey.findProgramAddressSync([new web3.PublicKey(PROGRAM_ID).toBuffer()], LOADER);
const auth = web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(process.argv[2], 'utf8'))));
const SEND = process.argv[3] === '--send';

async function state() {
  const raw = await conn.getAccountInfo(programData, 'confirmed');
  const d = raw.data, code = d.subarray(45);
  return {
    owner: raw.owner.toBase58(), tag: d.readUInt32LE(0), slot: Number(d.readBigUInt64LE(4)),
    authority: d[12] === 1 ? new web3.PublicKey(d.subarray(13, 45)).toBase58() : null,
    sha: crypto.createHash('sha256').update(code.subarray(0, BUILD_LEN)).digest('hex'),
    padZero: code.subarray(BUILD_LEN).every(b => b === 0),
  };
}
const before = await state();
console.log('programData', programData.toBase58());
console.log('before', JSON.stringify(before));
if (before.owner !== LOADER.toBase58() || before.tag !== 3) throw new Error('not an upgradeable ProgramData account');
if (before.sha !== BUILD || !before.padZero) throw new Error('deployed binary does not match the recorded build: refusing to freeze');
console.log('binary matches recorded build: true');
if (before.authority === null) { console.log('already frozen'); process.exit(0); }
if (before.authority !== auth.publicKey.toBase58()) throw new Error('supplied key is not the current upgrade authority');

const ix = new web3.TransactionInstruction({
  programId: LOADER,
  keys: [
    { pubkey: programData, isSigner: false, isWritable: true },    // the account whose authority changes
    { pubkey: auth.publicKey, isSigner: true, isWritable: false }, // the current authority
    // no third account: no new authority, so the program stops being upgradeable
  ],
  data: Buffer.from([4, 0, 0, 0]), // UpgradeableLoaderInstruction::SetAuthority
});
const tx = new web3.Transaction().add(ix);
tx.feePayer = auth.publicKey;
const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash('confirmed');
tx.recentBlockhash = blockhash;
tx.sign(auth);
const sim = await conn.simulateTransaction(tx);
console.log('simulation err:', JSON.stringify(sim.value.err));
for (const l of sim.value.logs || []) console.log('  sim |', l);
if (sim.value.err) process.exit(1);
if (!SEND) { console.log('SIMULATION ONLY: nothing sent'); process.exit(0); }

const sig = await conn.sendRawTransaction(tx.serialize());
console.log('sent', sig);
const res = await conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, 'confirmed');
console.log('confirmed err:', JSON.stringify(res.value.err));
const after = await state();
console.log('after', JSON.stringify(after));
console.log(after.authority === null ? 'FROZEN: upgrade authority is None' : 'NOT FROZEN');
