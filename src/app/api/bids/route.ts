import { userIdFrom } from "@/lib/privy-server";

// Do not accept orders until a durable matching and fulfillment service exists.
async function unavailable(request: Request) {
  if (!(await userIdFrom(request))) return Response.json({ error: "Sign in to access capacity orders." }, { status: 401 });
  return Response.json({ error: "Capacity auctions are not implemented. Use the inference router for prepaid requests." }, { status: 501 });
}

export { unavailable as GET, unavailable as POST };
