"use client";

import { useState } from "react";
import { users } from "@/lib/mock-data";
import type { UserSummary } from "@/lib/types";

function StatusBadge({ status }: { status: UserSummary["status"] }) {
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${
        status === "active"
          ? "bg-green-100 text-green-700"
          : "bg-red-100 text-red-700"
      }`}
    >
      {status === "active" ? "Active" : "Frozen"}
    </span>
  );
}

const KYC_LEVELS = [0, 1, 2, 3] as const;

function KycDropdown({
  value,
  onChange,
}: {
  value: UserSummary["kycLevel"];
  onChange: (level: UserSummary["kycLevel"]) => void;
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(Number(e.target.value) as UserSummary["kycLevel"])}
      className="border border-gray-300 rounded px-2 py-1 text-xs bg-white focus:outline-none focus:ring-1 focus:ring-blue-500"
    >
      {KYC_LEVELS.map((level) => (
        <option key={level} value={level}>
          L{level}
        </option>
      ))}
    </select>
  );
}

export default function UsersPage() {
  const [search, setSearch] = useState("");
  const [userList, setUserList] = useState(users);

  const filtered = userList.filter(
    (u) =>
      u.id.toLowerCase().includes(search.toLowerCase()) ||
      u.email.toLowerCase().includes(search.toLowerCase())
  );

  function toggleFreeze(id: string) {
    setUserList((prev) =>
      prev.map((u) =>
        u.id === id
          ? { ...u, status: u.status === "active" ? "frozen" : "active" as const }
          : u
      )
    );
  }

  function setKycLevel(id: string, level: UserSummary["kycLevel"]) {
    setUserList((prev) =>
      prev.map((u) => (u.id === id ? { ...u, kycLevel: level } : u))
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-4">
        <input
          type="text"
          placeholder="Search by ID or email..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="w-full max-w-sm px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
        />
        <span className="text-sm text-gray-500">
          {filtered.length} user{filtered.length !== 1 && "s"}
        </span>
      </div>

      <div className="bg-white rounded-lg border border-gray-200 shadow-sm overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-200 text-left text-gray-500 bg-gray-50">
              <th className="px-4 py-3 font-medium">User ID</th>
              <th className="px-4 py-3 font-medium">Email</th>
              <th className="px-4 py-3 font-medium">KYC Level</th>
              <th className="px-4 py-3 font-medium">Status</th>
              <th className="px-4 py-3 font-medium">Created</th>
              <th className="px-4 py-3 font-medium text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((u) => (
              <tr key={u.id} className="border-b border-gray-100 hover:bg-gray-50">
                <td className="px-4 py-3 font-mono text-xs">{u.id}</td>
                <td className="px-4 py-3">{u.email}</td>
                <td className="px-4 py-3">
                  <KycDropdown
                    value={u.kycLevel}
                    onChange={(level) => setKycLevel(u.id, level)}
                  />
                </td>
                <td className="px-4 py-3">
                  <StatusBadge status={u.status} />
                </td>
                <td className="px-4 py-3 text-gray-600">{u.createdAt}</td>
                <td className="px-4 py-3 text-right space-x-2">
                  <button className="text-blue-600 hover:text-blue-800 text-xs font-medium">
                    View
                  </button>
                  <button
                    onClick={() => toggleFreeze(u.id)}
                    className={`text-xs font-medium ${
                      u.status === "active"
                        ? "text-red-600 hover:text-red-800"
                        : "text-green-600 hover:text-green-800"
                    }`}
                  >
                    {u.status === "active" ? "Freeze" : "Unfreeze"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
