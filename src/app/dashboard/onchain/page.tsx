import { redirect } from "next/navigation";
export const instant = false;
// The on-chain market is now the Market.
export default function OnchainRedirect() { redirect("/dashboard/market"); }
