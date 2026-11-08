import DashboardShell from "@/components/DashboardShell";
import { AccountProvider } from "@/components/AccountProvider";

export default function DashboardLayout({ children }: LayoutProps<"/dashboard">) {
  return <AccountProvider><DashboardShell>{children}</DashboardShell></AccountProvider>;
}
