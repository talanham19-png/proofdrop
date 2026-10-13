#!/usr/bin/env node
// Read-only check of the deployed ProofDrop program. Needs no key.
//   node scripts/check-program.mjs
// Prints (1) whether the on-chain binary is byte-for-byte the build recorded in test/logs/build-hash.txt
// and (2) the program's upgrade authority. "none" means the program is frozen: nobody can change it.
import fs from 'node:fs';
import crypto from 'node:crypto';
import * as web3 from '@solana/web3.js';
import { PROGRAM_ID } from '../lib/escrow.mjs';

const conn = new web3.Connection(process.env.RPC_URL || web3.clusterApiUrl('devnet'), 'finalized');
const LOADER = new web3.PublicKey('BPFLoaderUpgradeab1e11111111111111111111111');
const BUILD_LEN = 112096; // bytes of program/target/deploy/proofdrop.so
const want = fs.readFileSync(new URL('../test/logs/build-hash.txt', import.meta.url), 'utf8').trim().split(/\s+/)[0];
const [programData] = web3.PublicKey.findProgramAddressSync([new web3.PublicKey(PROGRAM_ID).toBuffer()], LOADER);

const acct = await conn.getAccountInfo(programData);
if (!acct || !acct.owner.equals(LOADER) || acct.data.readUInt32LE(0) !== 3) { console.log('not an upgradeable program-data account'); process.exit(1); }
// ProgramData layout: u32 tag (3) | u64 last-deployed slot | Option<Pubkey> upgrade authority | program bytes
const d = acct.data;
const authority = d[12] === 1 ? new web3.PublicKey(d.subarray(13, 45)).toBase58() : null;
const code = d.subarray(45);
const got = crypto.createHash('sha256').update(code.subarray(0, BUILD_LEN)).digest('hex');
const padded = code.subarray(BUILD_LEN).every(b => b === 0);

console.log('program            ', PROGRAM_ID);
console.log('program data       ', programData.toBase58());
console.log('last deployed slot ', Number(d.readBigUInt64LE(4)));
console.log('recorded build     ', want);
console.log('on-chain binary    ', got, got === want && padded ? '(MATCH, rest is zero padding)' : '(DOES NOT MATCH)');
console.log('upgrade authority  ', authority ?? 'none (frozen: no one can change this program)');
process.exit(got === want && padded ? 0 : 1);
