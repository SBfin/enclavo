// This probe checks the private gateway and its database without making an inference request.
export async function GET(request: Request) {
  const headers = { "cache-control": "no-store" };
  if (!process.env.ENCLAVO_CONTROL_TOKEN) {
    return Response.json({ status: "unavailable" }, { status: 503, headers });
  }
  try {
    const url = new URL("/readyz", process.env.ENCLAVO_GATEWAY_URL || "http://127.0.0.1:8088");
    const response = await fetch(url, {
      cache: "no-store",
      redirect: "error",
      signal: AbortSignal.any([request.signal, AbortSignal.timeout(3_000)]),
    });
    if (!response.ok) throw new Error("Gateway unavailable");
    return Response.json({ status: "ready" }, { headers });
  } catch {
    return Response.json({ status: "unavailable" }, { status: 503, headers });
  }
}
