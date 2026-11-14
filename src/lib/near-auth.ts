import "server-only";
import { createHash } from "node:crypto";
import { SESSION_TTL_MS, readSession } from "@/lib/near-auth-core";

export const NEAR_NETWORK = process.env.NEXT_PUBLIC_NEAR_NETWORK === "mainnet" ? "mainnet" : "testnet";
export const NEAR_RPC_URL = process.env.NEAR_RPC_URL || (NEAR_NETWORK === "mainnet" ? "https://free.rpc.fastnear.com" : "https://test.rpc.fastnear.com");
const COOKIE = "enclavo_session";

// Shared secret for the dashboard → gateway channel. It also keys sign-in challenges and
// sessions, under separate labels, so the deployment needs no additional secret.
export function controlToken(): string | null {
  return process.env.ENCLAVO_CONTROL_TOKEN || null;
}

export function authKey(): Buffer | null {
  const token = controlToken();
  return token ? createHash("sha256").update(`enclavo:auth:v1:${token}`).digest() : null;
}

function cookieValue(request: Request, name: string): string | undefined {
  for (const part of (request.headers.get("cookie") || "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return undefined;
}

// The signed-in NEAR account for a request, or null.
export function sessionAccount(request: Request): string | null {
  const key = authKey();
  return key ? readSession(key, cookieValue(request, COOKIE)) : null;
}

// The identity the gateway stores as an account owner.
export function userIdFrom(request: Request): string | null {
  const account = sessionAccount(request);
  return account ? `near:${account}` : null;
}

export function sessionCookie(request: Request, token: string | null): string {
  const secure = new URL(request.url).protocol === "https:" || request.headers.get("x-forwarded-proto") === "https";
  const age = token ? Math.floor(SESSION_TTL_MS / 1000) : 0;
  return `${COOKIE}=${token ?? ""}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${age}${secure ? "; Secure" : ""}`;
}

// State-changing requests must come from this site, on top of the SameSite cookie.
export function sameOrigin(request: Request): boolean {
  if (["GET", "HEAD"].includes(request.method)) return true;
  const origin = request.headers.get("origin");
  if (!origin) return false;
  const host = request.headers.get("x-forwarded-host") || request.headers.get("host");
  try { return new URL(origin).host === host; } catch { return false; }
}
