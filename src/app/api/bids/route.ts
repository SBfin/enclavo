import { findRoute, TERMS } from "@/lib/market";
import { userIdFrom } from "@/lib/privy-server";

export type Bid = {
  id: string;
  userId: string;
  routeId: string;
  iu: number;
  days: number;
  maxPrice: number;
  createdAt: string;
};

// Prototype store: kept in memory, so bids reset when the server restarts.
const store = globalThis as unknown as { __enclavoBids?: Bid[] };
const bids = (store.__enclavoBids ??= []);

export async function GET(request: Request) {
  const userId = await userIdFrom(request);
  if (!userId) return Response.json({ error: "Sign in to see your bids." }, { status: 401 });
  return Response.json({ bids: bids.filter((b) => b.userId === userId).reverse() });
}

export async function POST(request: Request) {
  const userId = await userIdFrom(request);
  if (!userId) return Response.json({ error: "Sign in to place a bid." }, { status: 401 });

  const body = (await request.json().catch(() => null)) as Partial<Bid> | null;
  const route = body?.routeId ? findRoute(body.routeId) : undefined;
  const iu = Number(body?.iu);
  const days = Number(body?.days);
  const maxPrice = Number(body?.maxPrice);

  if (!route) return Response.json({ error: "Choose a route." }, { status: 400 });
  if (!Number.isInteger(iu) || iu < 10 || iu % 10 !== 0)
    return Response.json({ error: "Quantity must be a multiple of 10 IU." }, { status: 400 });
  if (!(TERMS as readonly number[]).includes(days))
    return Response.json({ error: "Choose a 7, 30 or 90-day term." }, { status: 400 });
  if (!(maxPrice > 0)) return Response.json({ error: "Set a maximum price above zero." }, { status: 400 });

  const bid: Bid = {
    id: crypto.randomUUID(),
    userId,
    routeId: route.id,
    iu,
    days,
    maxPrice,
    createdAt: new Date().toISOString(),
  };
  bids.push(bid);
  return Response.json({ bid }, { status: 201 });
}
