import { dashboardRequest } from "@/lib/dashboard-server";
async function bids(request: Request) { return dashboardRequest(request, "market/bids"); }
export { bids as GET, bids as POST };
