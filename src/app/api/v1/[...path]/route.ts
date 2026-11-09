import { forwardToGateway } from "@/lib/gateway-http";

async function proxy(request: Request, context: RouteContext<"/api/v1/[...path]">) {
  const { path } = await context.params;
  return forwardToGateway(request, process.env.ENCLAVO_GATEWAY_URL || "http://127.0.0.1:8088", `/v1/${path.map(encodeURIComponent).join("/")}`);
}

export { proxy as GET, proxy as POST };
