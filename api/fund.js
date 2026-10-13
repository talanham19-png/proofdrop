// Devnet starter faucet: sends a new ProofDrop wallet enough SOL for about ten entries,
// so a first-time player never has to leave the page. Devnet only.
// Zero dependencies: builds and signs the one System transfer with Node's built-in Ed25519.
// The faucet key lives in the FAUCET_SECRET env var (a 64-number JSON secret-key array).
// A deployment without that variable forwards the request to KEYED_FAUCET, a deployment that has it.
import crypto from 'node:crypto';

const RPC = process.env.RPC_URL || 'https://api.devnet.solana.com';
const DRIP = 20_000_000;    // 0.02 SOL
const ENOUGH = 5_000_000;   // wallets above 0.005 SOL are already funded
const COOLDOWN_MS = 30_000;
const recent = new Map();
const KEYED_FAUCET = 'https://proofdrop-qm0oqs9fv-talanham19-pngs-projects.vercel.app/api/fund';

const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function b58decode(s) {
  let n = 0n;
  for (const c of s) { const i = B58.indexOf(c); if (i < 0) throw new Error('bad base58'); n = n * 58n + BigInt(i); }
  const bytes = []; while (n > 0n) { bytes.unshift(Number(n % 256n)); n /= 256n; }
  for (const c of s) { if (c === '1') bytes.unshift(0); else break; }
  return Uint8Array.from(bytes);
}
function b58encode(u8) {
  let n = 0n; for (const b of u8) n = n * 256n + BigInt(b);
  let s = ''; while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
  for (const b of u8) { if (b === 0) s = '1' + s; else break; }
  return s;
}
const compact = n => { const out = []; do { let b = n & 0x7f; n >>= 7; if (n) b |= 0x80; out.push(b); } while (n); return out; };
async function rpc(method, params) {
  let last;
  for (let attempt = 0; attempt < 3; attempt++) { // public devnet RPC rate-limits shared IPs; retry with backoff
    try {
      const r = await fetch(RPC, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) });
      if (r.status === 429 || r.status >= 500) throw new Error('RPC ' + r.status);
      const j = await r.json();
      if (j.error) throw new Error(j.error.message || JSON.stringify(j.error));
      return j.result;
    } catch (e) { last = e; await new Promise(res => setTimeout(res, 600 * (attempt + 1))); }
  }
  throw last;
}

export default async function handler(req, res) {
  res.setHeader('Cache-Control', 'no-store');
  if (req.method !== 'POST') return res.status(405).json({ error: 'POST only' });
  let dest;
  try {
    const body = typeof req.body === 'string' ? JSON.parse(req.body) : req.body;
    dest = b58decode(String(body?.address || ''));
    if (dest.length !== 32) throw new Error('length');
  } catch { return res.status(400).json({ error: 'Send {"address": "<a Solana wallet address>"}.' }); }
  const destStr = b58encode(dest);

  const ip = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim() || 'unknown';
  const now = Date.now();
  for (const k of [ip, destStr]) {
    if (now - (recent.get(k) || 0) < COOLDOWN_MS) return res.status(429).json({ error: 'One drip every 30 seconds. Try again shortly.' });
  }
  if (!process.env.FAUCET_SECRET) {
    // This deployment has no faucet key of its own, so it hands the request to the deployment that holds one.
    try {
      const up = await fetch(KEYED_FAUCET, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ address: destStr }) });
      const out = await up.json();
      if (up.ok && out.funded) { recent.set(ip, now); recent.set(destStr, now); }
      return res.status(up.status).json(out);
    } catch (e) {
      return res.status(503).json({ error: 'Faucet not configured.' });
    }
  }

  try {
    const balance = (await rpc('getBalance', [destStr, { commitment: 'confirmed' }])).value;
    if (balance >= ENOUGH) return res.status(200).json({ funded: false, balance, note: 'Already funded.' });

    const secret = Uint8Array.from(JSON.parse(process.env.FAUCET_SECRET));
    const seed = secret.slice(0, 32), from = secret.slice(32, 64);
    const key = crypto.createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), Buffer.from(seed)]), format: 'der', type: 'pkcs8' });
    const { blockhash } = (await rpc('getLatestBlockhash', [{ commitment: 'confirmed' }])).value;

    const data = Buffer.alloc(12); data.writeUInt32LE(2, 0); data.writeBigUInt64LE(BigInt(DRIP), 4); // System: Transfer
    const message = Buffer.from([
      1, 0, 1,                                   // 1 signer, 0 readonly signed, 1 readonly unsigned
      ...compact(3), ...from, ...dest, ...new Uint8Array(32), // faucet, recipient, System Program (all zeros)
      ...b58decode(blockhash),
      ...compact(1), 2, ...compact(2), 0, 1, ...compact(data.length), ...data,
    ]);
    const sig = crypto.sign(null, message, key);
    const tx = Buffer.concat([Buffer.from(compact(1)), sig, message]);

    recent.set(ip, now); recent.set(destStr, now);
    const signature = await rpc('sendTransaction', [tx.toString('base64'), { encoding: 'base64', preflightCommitment: 'confirmed' }]);
    for (let i = 0; i < 30; i++) {
      const st = (await rpc('getSignatureStatuses', [[signature]])).value[0];
      if (st?.err) throw new Error('transfer failed on chain');
      if (st && (st.confirmationStatus === 'confirmed' || st.confirmationStatus === 'finalized')) break;
      await new Promise(r => setTimeout(r, 700));
    }
    return res.status(200).json({ funded: true, lamports: DRIP, signature });
  } catch (e) {
    return res.status(502).json({ error: 'Faucet transfer failed: ' + (e?.message || e) });
  }
}
