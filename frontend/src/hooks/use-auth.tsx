/* eslint-disable react-refresh/only-export-components */
import React, { createContext, useContext, useEffect, useState } from "react";
import { api, subscribeDashboardUnauthorized } from "@/lib/api";
import type { User } from "@/lib/api";
import { clearCache } from "@/lib/swr";

interface AuthContextType {
  user: User | null;
  loading: boolean;
  login: (username: string, password: string, captchaToken: string) => Promise<void>;
  register: (username: string, password: string, captchaToken: string) => Promise<void>;
  changePassword: (currentPassword: string, newPassword: string) => Promise<void>;
  logout: () => Promise<void>;
  refreshUser: () => Promise<void>;
}

const AuthContext = createContext<AuthContextType | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);

  const refreshUser = async () => {
    try {
      const userData = await api.me();
      setUser(userData);
    } catch {
      setUser(null);
      await clearCache();
    }
  };

  useEffect(() => {
    return subscribeDashboardUnauthorized(() => {
      setUser(null);
      void clearCache();
    });
  }, []);

  useEffect(() => {
    refreshUser().finally(() => setLoading(false));
  }, []);

  const login = async (username: string, password: string, captchaToken: string) => {
    const response = await api.login(username, password, captchaToken);
    await clearCache();
    setUser(response.user);
  };

  const register = async (username: string, password: string, captchaToken: string) => {
    const response = await api.register(username, password, captchaToken);
    await clearCache();
    setUser(response.user);
  };

  const changePassword = async (currentPassword: string, newPassword: string) => {
    const response = await api.changePassword(currentPassword, newPassword);
    await clearCache();
    setUser(response.user);
  };

  const logout = async () => {
    try {
      await api.logout();
    } finally {
      setUser(null);
      await clearCache();
    }
  };

  return (
    <AuthContext.Provider
      value={{ user, loading, login, register, changePassword, logout, refreshUser }}
    >
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
}
