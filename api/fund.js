// Devnet starter faucet: sends a new ProofDrop wallet enough SOL for a few
// hundred transactions, so a first-time player never has to leave the page.
// Devnet only. The faucet key lives in the FAUCET_SECRET env var (JSON array).
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, sendAndConfirmTransaction, LAMPORTS_PER_SOL } from '@solana/web3.js';

const RPC = process.env.RPC_URL || 'https://api.devnet.solana.com';
const DRIP = Math.round(0.02 * LAMPORTS_PER_SOL);   // ~4,000 memo transactions
const ENOUGH = Math.round(0.005 * LAMPORTS_PER_SOL); // wallets above this are already funded
const COOLDOWN_MS = 30_000;
const recent = new Map(); // best-effort per-instance rate limit: key -> last drip time

export default async function handler(req, res) {
  res.setHeader('Cache-Control', 'no-store');
  if (req.method !== 'POST') return res.status(405).json({ error: 'POST only' });
  let address;
  try {
    const body = typeof req.body === 'string' ? JSON.parse(req.body) : req.body;
    address = new PublicKey(String(body?.address || ''));
    if (!PublicKey.isOnCurve(address.toBytes())) throw new Error('off-curve');
  } catch { return res.status(400).json({ error: 'Send {"address": "<a Solana wallet address>"}.' }); }

  const ip = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim() || 'unknown';
  const now = Date.now();
  for (const k of [ip, address.toBase58()]) {
    if (now - (recent.get(k) || 0) < COOLDOWN_MS) return res.status(429).json({ error: 'One drip every 30 seconds. Try again shortly.' });
  }

  if (!process.env.FAUCET_SECRET) return res.status(503).json({ error: 'Faucet not configured.' });
  const conn = new Connection(RPC, 'confirmed');
  try {
    const balance = await conn.getBalance(address, 'confirmed');
    if (balance >= ENOUGH) return res.status(200).json({ funded: false, balance, note: 'Already funded.' });
    const faucet = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(process.env.FAUCET_SECRET)));
    const tx = new Transaction().add(SystemProgram.transfer({ fromPubkey: faucet.publicKey, toPubkey: address, lamports: DRIP }));
    recent.set(ip, now); recent.set(address.toBase58(), now);
    const signature = await sendAndConfirmTransaction(conn, tx, [faucet], { commitment: 'confirmed' });
    return res.status(200).json({ funded: true, lamports: DRIP, signature });
  } catch (e) {
    return res.status(502).json({ error: 'Faucet transfer failed: ' + (e?.message || e) });
  }
}
