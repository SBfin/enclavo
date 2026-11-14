import { SIGN_IN_MESSAGE, SIGN_IN_RECIPIENT, makeChallengeNonce } from "@/lib/near-auth-core";
import { authKey, sameOrigin } from "@/lib/near-auth";

// A short-lived, single-use nonce for the wallet to sign (NEP-413).
export function POST(request: Request) {
  const key = authKey();
  if (!key) return Response.json({ error: { message: "Sign-in is not configured." } }, { status: 503 });
  if (!sameOrigin(request)) return Response.json({ error: { message: "Cross-site request refused." } }, { status: 403 });
  return Response.json(
    { message: SIGN_IN_MESSAGE, recipient: SIGN_IN_RECIPIENT, nonce: makeChallengeNonce(key).toString("base64") },
    { headers: { "cache-control": "no-store" } },
  );
}
