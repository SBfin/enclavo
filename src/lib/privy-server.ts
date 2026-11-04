import "server-only";
import { PrivyClient } from "@privy-io/node";

let client: PrivyClient | null = null;

function privy(): PrivyClient {
  if (client) return client;
  const appId = process.env.NEXT_PUBLIC_PRIVY_APP_ID;
  const appSecret = process.env.PRIVY_APP_SECRET;
  if (!appId || !appSecret) throw new Error("Privy credentials are not configured");
  client = new PrivyClient({ appId, appSecret });
  return client;
}

// Returns the Privy user ID for a request's bearer token, or null if it is missing or invalid.
export async function userIdFrom(request: Request): Promise<string | null> {
  const token = request.headers.get("authorization")?.replace(/^Bearer\s+/i, "");
  if (!token) return null;
  try {
    const claims = await privy().utils().auth().verifyAccessToken(token);
    return claims.user_id;
  } catch {
    return null;
  }
}
