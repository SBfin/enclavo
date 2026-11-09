import { dashboardRequest } from "@/lib/dashboard-server";

async function handler(request: Request, context: RouteContext<"/api/dashboard/[...path]">) {
  const { path } = await context.params;
  const value = path.map(encodeURIComponent).join("/");
  if (!/^(account(?:\/import)?|keys(?:\/[a-f0-9-]{36})?|chat\/completions|market\/.*)$/.test(value)) {
    return Response.json({ error: { message: "Unknown account operation." } }, { status: 404 });
  }
  return dashboardRequest(request, value);
}
export { handler as GET, handler as POST, handler as DELETE };
