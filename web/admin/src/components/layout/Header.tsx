"use client";

import { usePathname } from "next/navigation";

const pageTitles: Record<string, string> = {
  "/": "Dashboard",
  "/users": "User Management",
  "/risk": "Risk Monitoring",
  "/assets": "Asset Reports",
  "/symbols": "Symbol Management",
  "/system": "System Monitoring",
};

export default function Header() {
  const pathname = usePathname();
  const title = pageTitles[pathname] ?? "EXG Admin";

  return (
    <header className="flex items-center justify-between h-14 px-6 bg-white border-b border-gray-200">
      <h1 className="text-lg font-semibold text-gray-900">{title}</h1>
      <div className="flex items-center gap-3">
        <span className="text-sm text-gray-500">Admin</span>
        <div className="w-8 h-8 rounded-full bg-slate-700 flex items-center justify-center text-white text-xs font-medium">
          A
        </div>
      </div>
    </header>
  );
}
