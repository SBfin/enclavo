import "server-only";
import { createHash } from "node:crypto";
import { userIdFrom } from "@/lib/privy-server";

export async function dashboardRequest(request: Request, path: string): Promise<Response> {
  const userId = await userIdFrom(request);
  if (!userId) return Response.json({ error: { message: "Sign in to access your Enclavo account." } }, { status: 401 });
  const secret = process.env.PRIVY_APP_SECRET;
  const token = process.env.ENCLAVO_CONTROL_TOKEN || (secret ? createHash("sha256").update(`enclavo:control:v1:${secret}`).digest("hex") : null);
  if (!token) return Response.json({ error: { message: "Account management is not configured." } }, { status: 503 });
  const headers = new Headers({ authorization: `Bearer ${token}`, "x-enclavo-user-id": userId });
  for (const name of ["content-type", "accept"]) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  try {
    const url = new URL(`/internal/${path}`, process.env.ENCLAVO_GATEWAY_URL || "http://127.0.0.1:8088");
    url.search = new URL(request.url).search;
    const init: RequestInit & { duplex?: "half" } = { method: request.method, headers, cache: "no-store", signal: AbortSignal.any([request.signal, AbortSignal.timeout(600_000)]) };
    if (!["GET", "HEAD"].includes(request.method)) { init.body = request.body; init.duplex = "half"; }
    const response = await fetch(url, init);
    const outgoing = new Headers({ "cache-control": "private, no-store" });
    for (const [name, value] of response.headers) {
      if (["content-type", "retry-after", "x-request-id"].includes(name) || name.startsWith("x-enclavo-")) outgoing.set(name, value);
    }
    if (outgoing.get("content-type")?.includes("text/event-stream")) outgoing.set("x-accel-buffering", "no");
    return new Response(response.body, { status: response.status, headers: outgoing });
  } catch {
    return Response.json({ error: { message: "The account service is unavailable. Please retry shortly." } }, { status: 503 });
  }
}
