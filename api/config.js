// Serves the keyed devnet RPC URL from the HELIUS_RPC env var, so the key never lives in the repo.
// The page falls back to the public devnet RPC when this is unset or the key is refused.
export default function handler(req, res) {
  res.setHeader('Cache-Control', 'public, max-age=300');
  res.status(200).json({ devnet: process.env.HELIUS_RPC || null });
}
