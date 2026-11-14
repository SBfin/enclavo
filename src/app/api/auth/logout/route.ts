import { sameOrigin, sessionCookie } from "@/lib/near-auth";

export function POST(request: Request) {
  if (!sameOrigin(request)) return Response.json({ error: { message: "Cross-site request refused." } }, { status: 403 });
  return new Response(null, { status: 204, headers: { "set-cookie": sessionCookie(request, null) } });
}
