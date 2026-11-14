// NEAR wallet sign-in: NEP-413 signed messages, stateless challenges and signed session tokens.
// Depends only on node:crypto so the Node test runner can exercise it directly.
import { createHash, createHmac, createPublicKey, randomBytes, timingSafeEqual, verify } from "node:crypto";

export const SIGN_IN_MESSAGE = "Sign in to Enclavo.\n\nThis proves you control this NEAR account. It sends no transaction and costs nothing.";
export const SIGN_IN_RECIPIENT = "enclavo";
const CHALLENGE_TTL_MS = 5 * 60_000;
export const SESSION_TTL_MS = 7 * 24 * 60 * 60_000;

// NEAR account IDs: 2–64 chars of lowercase alphanumeric parts joined by ., - or _ (implicit accounts are 64 hex chars).
export function isAccountId(value: unknown): value is string {
  return typeof value === "string" && value.length >= 2 && value.length <= 64 && /^(([a-z\d]+[-_])*[a-z\d]+\.)*([a-z\d]+[-_])*[a-z\d]+$/.test(value);
}

const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
export function base58Decode(text: string): Buffer {
  let value = BigInt(0);
  for (const char of text) {
    const digit = BASE58.indexOf(char);
    if (digit < 0) throw new Error("Invalid base58");
    value = value * BigInt(58) + BigInt(digit);
  }
  const bytes: number[] = [];
  while (value > BigInt(0)) { bytes.unshift(Number(value % BigInt(256))); value /= BigInt(256); }
  for (const char of text) { if (char !== "1") break; bytes.unshift(0); }
  return Buffer.from(bytes);
}

function borshString(text: string): Buffer {
  const bytes = Buffer.from(text, "utf8");
  const length = Buffer.alloc(4);
  length.writeUInt32LE(bytes.length);
  return Buffer.concat([length, bytes]);
}

// The bytes a wallet signs for NEP-413: sha256(u32 tag 2^31+413 || borsh(payload)).
export function nep413Hash(message: string, nonce: Buffer, recipient: string): Buffer {
  const tag = Buffer.alloc(4);
  tag.writeUInt32LE(2 ** 31 + 413);
  const payload = Buffer.concat([borshString(message), nonce, borshString(recipient), Buffer.from([0])]);
  return createHash("sha256").update(Buffer.concat([tag, payload])).digest();
}

function decodeSignature(signature: string): Buffer | null {
  const raw = signature.replace(/^ed25519:/, "");
  const fromBase64 = Buffer.from(raw, "base64");
  if (fromBase64.length === 64) return fromBase64;
  try { const fromBase58 = base58Decode(raw); return fromBase58.length === 64 ? fromBase58 : null; } catch { return null; }
}

// Checks that `signature` is publicKey's ed25519 signature over the NEP-413 payload.
export function verifyNep413(input: { publicKey: string; signature: string; message: string; nonce: Buffer; recipient: string }): boolean {
  if (!input.publicKey.startsWith("ed25519:") || input.nonce.length !== 32) return false;
  try {
    const raw = base58Decode(input.publicKey.slice("ed25519:".length));
    const signature = decodeSignature(input.signature);
    if (raw.length !== 32 || !signature) return false;
    const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw]), format: "der", type: "spki" });
    return verify(null, nep413Hash(input.message, input.nonce, input.recipient), key, signature);
  } catch {
    return false;
  }
}

const mac = (key: Buffer, label: string, data: Buffer | string) => createHmac("sha256", key).update(label).update(data).digest();

// A 32-byte nonce the server can check without storing it: expiry (8) || random (8) || HMAC (16).
export function makeChallengeNonce(key: Buffer, now = Date.now()): Buffer {
  const head = Buffer.alloc(16);
  head.writeBigUInt64BE(BigInt(now + CHALLENGE_TTL_MS));
  randomBytes(8).copy(head, 8);
  return Buffer.concat([head, mac(key, "enclavo:challenge:v1", head).subarray(0, 16)]);
}

export function checkChallengeNonce(key: Buffer, nonce: Buffer, now = Date.now()): boolean {
  if (nonce.length !== 32) return false;
  const head = nonce.subarray(0, 16);
  const expected = mac(key, "enclavo:challenge:v1", head).subarray(0, 16);
  return timingSafeEqual(expected, nonce.subarray(16)) && Number(head.readBigUInt64BE()) > now;
}

// Session token: base64url(json) "." base64url(HMAC).
export function signSession(key: Buffer, accountId: string, now = Date.now()): string {
  const body = Buffer.from(JSON.stringify({ a: accountId, e: now + SESSION_TTL_MS })).toString("base64url");
  return `${body}.${mac(key, "enclavo:session:v1", body).toString("base64url")}`;
}

export function readSession(key: Buffer, token: string | undefined, now = Date.now()): string | null {
  if (!token) return null;
  const [body, signature] = token.split(".");
  if (!body || !signature) return null;
  const expected = mac(key, "enclavo:session:v1", body);
  const given = Buffer.from(signature, "base64url");
  if (given.length !== expected.length || !timingSafeEqual(given, expected)) return null;
  try {
    const { a, e } = JSON.parse(Buffer.from(body, "base64url").toString("utf8"));
    return isAccountId(a) && typeof e === "number" && e > now ? a : null;
  } catch {
    return null;
  }
}

// True when publicKey is a full-access key of accountId, per the chain.
export async function hasFullAccessKey(rpcUrl: string, accountId: string, publicKey: string, fetchImpl: typeof fetch = fetch): Promise<boolean> {
  try {
    const response = await fetchImpl(rpcUrl, {
      method: "POST",
      headers: { "content-type": "application/json" },
      signal: AbortSignal.timeout(8_000),
      body: JSON.stringify({ jsonrpc: "2.0", id: "enclavo", method: "query", params: { request_type: "view_access_key", finality: "final", account_id: accountId, public_key: publicKey } }),
    });
    const body = await response.json();
    return body?.result?.permission === "FullAccess";
  } catch {
    return false;
  }
}
