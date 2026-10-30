/* eslint-disable react-refresh/only-export-components */
import React, { createContext, useContext, useEffect, useState } from "react";
import { mutate as globalMutate } from "swr";
import { api, subscribeDashboardUnauthorized } from "@/lib/api";
import type { User } from "@/lib/api";
import { clearAuthenticatedCache, SWR_KEYS, useCurrentUser } from "@/lib/swr";

const USER_REFRESH_INTERVAL_MS = 60000;

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
  // isPaused stops polling and focus revalidation after logout or a 401 (DSA9b),
  // so the login page does not keep probing /auth/me. The key stays subscribed
  // so login can write the new user straight into the cache.
  const [sessionActive, setSessionActive] = useState(true);
  const { data, error, mutate } = useCurrentUser({
    isPaused: () => !sessionActive,
    refreshInterval: USER_REFRESH_INTERVAL_MS,
  });
  const user = sessionActive ? (data ?? null) : null;
  const loading = sessionActive && data === undefined && !error;

  useEffect(() => {
    return subscribeDashboardUnauthorized(() => {
      setSessionActive(false);
      void clearAuthenticatedCache();
    });
  }, []);

  const adoptSession = async (next: User) => {
    await clearAuthenticatedCache({ keepCurrentUser: true });
    await globalMutate(SWR_KEYS.ME, next, { revalidate: false });
    setSessionActive(true);
  };

  const refreshUser = async () => {
    await mutate();
  };

  const login = async (username: string, password: string, captchaToken: string) => {
    const response = await api.login(username, password, captchaToken);
    await adoptSession(response.user);
  };

  const register = async (username: string, password: string, captchaToken: string) => {
    const response = await api.register(username, password, captchaToken);
    await adoptSession(response.user);
  };

  const changePassword = async (currentPassword: string, newPassword: string) => {
    const response = await api.changePassword(currentPassword, newPassword);
    await adoptSession(response.user);
  };

  const logout = async () => {
    try {
      await api.logout();
    } finally {
      setSessionActive(false);
      await clearAuthenticatedCache();
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
