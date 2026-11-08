import { redirect } from "next/navigation";

// This segment only redirects, so there is no UI to validate for instant navigation.
export const instant = false;

export default function DashboardIndex() {
  redirect("/dashboard/market");
}
