const REQUEST_HEADERS = ["authorization", "payment-signature", "content-type", "accept", "x-request-id"];

// Relay credentials and response bodies without caching, buffering streams, or forwarding cookies.
export async function forwardToGateway(request: Request, baseUrl: string, path: string): Promise<Response> {
  if (!path.startsWith("/v1/") || path.split("/").some((part) => [".", ".."].includes(decodeURIComponent(part)))) {
    return Response.json({ error: { message: "Invalid gateway path." } }, { status: 400 });
  }
  const headers = new Headers();
  for (const name of REQUEST_HEADERS) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  try {
    const url = new URL(path, baseUrl);
    url.search = new URL(request.url).search;
    const init: RequestInit & { duplex?: "half" } = {
      method: request.method, headers, cache: "no-store", redirect: "manual",
      signal: AbortSignal.any([request.signal, AbortSignal.timeout(600_000)]),
    };
    if (request.method !== "GET" && request.method !== "HEAD") {
      init.body = request.body;
      init.duplex = "half";
    }
    const upstream = await fetch(url, init);
    const responseHeaders = new Headers({ "cache-control": "private, no-store" });
    for (const [name, value] of upstream.headers) {
      if (name.startsWith("x-enclavo-") || ["content-type", "payment-required", "payment-response", "x-request-id", "retry-after"].includes(name)) {
        responseHeaders.set(name, value);
      }
    }
    if (upstream.headers.get("content-type")?.includes("text/event-stream")) {
      responseHeaders.set("x-accel-buffering", "no");
    }
    return new Response(upstream.body, { status: upstream.status, headers: responseHeaders });
  } catch {
    return Response.json({ error: { message: "The inference gateway is unavailable. Check its configuration and logs.", code: "gateway_unavailable" } }, { status: 503 });
  }
}
