import { SIGN_IN_MESSAGE, SIGN_IN_RECIPIENT, checkChallengeNonce, hasFullAccessKey, isAccountId, signSession, verifyNep413 } from "@/lib/near-auth-core";
import { NEAR_RPC_URL, authKey, sameOrigin, sessionCookie } from "@/lib/near-auth";

// Nonces already exchanged for a session. In memory: enough for one web instance, and each
// nonce expires within minutes anyway.
const used = (globalThis as unknown as { __enclavoNonces?: Map<string, number> }).__enclavoNonces ??= new Map();

export async function POST(request: Request) {
  const key = authKey();
  if (!key) return Response.json({ error: { message: "Sign-in is not configured." } }, { status: 503 });
  if (!sameOrigin(request)) return Response.json({ error: { message: "Cross-site request refused." } }, { status: 403 });
  const body = (await request.json().catch(() => null)) as Record<string, unknown> | null;
  const { accountId, publicKey, signature, nonce } = body ?? {};
  const fail = (message: string, status = 401) => Response.json({ error: { message } }, { status });
  if (!isAccountId(accountId) || typeof publicKey !== "string" || typeof signature !== "string" || typeof nonce !== "string")
    return fail("The wallet did not return a complete signed message.", 400);

  const nonceBytes = Buffer.from(nonce, "base64");
  const now = Date.now();
  for (const [value, expires] of used) if (expires < now) used.delete(value);
  if (!checkChallengeNonce(key, nonceBytes, now) || used.has(nonce)) return fail("This sign-in request expired. Try again.");
  if (!verifyNep413({ publicKey, signature, message: SIGN_IN_MESSAGE, nonce: nonceBytes, recipient: SIGN_IN_RECIPIENT }))
    return fail("The signature did not verify.");
  if (!(await hasFullAccessKey(NEAR_RPC_URL, accountId, publicKey)))
    return fail("That key is not a full-access key of this NEAR account.");

  used.set(nonce, now + 10 * 60_000);
  return Response.json({ accountId }, { headers: { "set-cookie": sessionCookie(request, signSession(key, accountId)), "cache-control": "no-store" } });
}
