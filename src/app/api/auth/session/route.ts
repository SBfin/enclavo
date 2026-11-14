import { connection } from "next/server";
import { sessionAccount } from "@/lib/near-auth";

// Per-request: the answer depends on the visitor's cookie, so it must never be prerendered.
export async function GET(request: Request) {
  await connection();
  return Response.json({ accountId: sessionAccount(request) }, { headers: { "cache-control": "private, no-store" } });
}
