import { redirect } from "next/navigation";
export const instant = false;
export default function RouterRedirect() { redirect("/dashboard/playground"); }
