"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { cn } from "@/lib/utils";
import Header from "@/components/layout/Header";

const tabs = [
  { href: "/account", label: "Overview" },
  { href: "/account/orders", label: "Orders" },
  { href: "/account/security", label: "Security" },
] as const;

export default function AccountLayout({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();

  return (
    <div className="flex flex-col h-screen">
      <Header />
      <div className="flex items-center gap-6 px-6 border-b border-border bg-card">
        {tabs.map((tab) => (
          <Link
            key={tab.href}
            href={tab.href}
            className={cn(
              "py-3 text-sm font-medium border-b-2 transition-colors",
              pathname === tab.href
                ? "border-accent text-primary"
                : "border-transparent text-secondary hover:text-primary"
            )}
          >
            {tab.label}
          </Link>
        ))}
      </div>
      <main className="flex-1 min-h-0 overflow-auto p-6">{children}</main>
    </div>
  );
}
