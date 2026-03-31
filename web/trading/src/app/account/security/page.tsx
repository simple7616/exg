"use client";

import { useState } from "react";
import { cn } from "@/lib/utils";

interface ApiKey {
  id: string;
  label: string;
  key: string;
  permissions: string[];
  createdAt: string;
}

const mockApiKeys: ApiKey[] = [
  { id: "1", label: "Trading Bot", key: "exg_k1_****abcd", permissions: ["trade", "read"], createdAt: "2026-03-15" },
  { id: "2", label: "Read Only", key: "exg_k2_****ef01", permissions: ["read"], createdAt: "2026-03-20" },
];

export default function SecurityPage() {
  const [twoFAEnabled, setTwoFAEnabled] = useState(false);
  const [apiKeys, setApiKeys] = useState(mockApiKeys);
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");

  function handleRevokeKey(id: string) {
    setApiKeys((prev) => prev.filter((k) => k.id !== id));
  }

  function handleCreateKey() {
    const newKey: ApiKey = {
      id: String(Date.now()),
      label: `Key ${apiKeys.length + 1}`,
      key: `exg_k${apiKeys.length + 1}_****${Math.random().toString(36).slice(2, 6)}`,
      permissions: ["trade", "read"],
      createdAt: new Date().toISOString().split("T")[0],
    };
    setApiKeys((prev) => [...prev, newKey]);
  }

  return (
    <div className="space-y-6 max-w-2xl">
      {/* Change Password */}
      <div className="bg-card rounded border border-border p-4 space-y-3">
        <h2 className="text-sm font-semibold text-primary">Change Password</h2>
        <div className="space-y-2">
          <div>
            <label className="text-[10px] text-secondary block mb-1">Current Password</label>
            <input
              type="password"
              value={oldPassword}
              onChange={(e) => setOldPassword(e.target.value)}
              className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm text-primary outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="text-[10px] text-secondary block mb-1">New Password</label>
            <input
              type="password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm text-primary outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="text-[10px] text-secondary block mb-1">Confirm New Password</label>
            <input
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              className="w-full bg-white/5 border border-border rounded px-3 py-2 text-sm text-primary outline-none focus:border-accent"
            />
          </div>
        </div>
        <button className="bg-accent text-black px-4 py-2 rounded text-sm font-semibold hover:bg-accent/80 transition-colors">
          Update Password
        </button>
      </div>

      {/* 2FA Setup */}
      <div className="bg-card rounded border border-border p-4 space-y-3">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold text-primary">Two-Factor Authentication</h2>
          <button
            onClick={() => setTwoFAEnabled(!twoFAEnabled)}
            className={cn(
              "relative w-10 h-5 rounded-full transition-colors",
              twoFAEnabled ? "bg-green" : "bg-white/20"
            )}
          >
            <span
              className={cn(
                "absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform",
                twoFAEnabled ? "translate-x-5" : "translate-x-0.5"
              )}
            />
          </button>
        </div>
        <p className="text-xs text-secondary">
          {twoFAEnabled
            ? "2FA is enabled. Your account is protected with an authenticator app."
            : "Enable 2FA to add an extra layer of security to your account using an authenticator app."}
        </p>
      </div>

      {/* API Key Management */}
      <div className="bg-card rounded border border-border">
        <div className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-primary">API Keys</h2>
          <button
            onClick={handleCreateKey}
            className="text-xs text-accent hover:text-accent/80 font-medium"
          >
            + Create Key
          </button>
        </div>
        {apiKeys.length === 0 ? (
          <p className="px-4 py-8 text-center text-secondary text-sm">No API keys</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-secondary text-xs">
                <th className="text-left px-4 py-2 font-medium">Label</th>
                <th className="text-left px-4 py-2 font-medium">Key</th>
                <th className="text-left px-4 py-2 font-medium">Permissions</th>
                <th className="text-left px-4 py-2 font-medium">Created</th>
                <th className="text-right px-4 py-2 font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {apiKeys.map((k) => (
                <tr key={k.id} className="border-b border-border/50 hover:bg-white/[0.02]">
                  <td className="px-4 py-2 text-primary">{k.label}</td>
                  <td className="px-4 py-2 font-mono text-xs text-secondary">{k.key}</td>
                  <td className="px-4 py-2 text-xs text-secondary">{k.permissions.join(", ")}</td>
                  <td className="px-4 py-2 text-xs text-secondary">{k.createdAt}</td>
                  <td className="px-4 py-2 text-right">
                    <button
                      onClick={() => handleRevokeKey(k.id)}
                      className="text-xs text-red hover:text-red/80 font-medium"
                    >
                      Revoke
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
