import { afterEach, describe, expect, test } from "bun:test";
import { api, subscribeDashboardUnauthorized } from "../src/lib/api";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("dashboard session transport", () => {
  test("includes cookies on dashboard requests", async () => {
    let credentials: RequestCredentials | undefined;
    globalThis.fetch = (async (_input, init) => {
      credentials = init?.credentials;
      return Response.json({ id: "user-1" });
    }) as typeof fetch;

    await api.me();

    expect(credentials).toBe("include");
  });

  test("invalidates browser auth state for an unauthorized dashboard session", async () => {
    globalThis.fetch = (async () =>
      Response.json(
        { error: { code: "unauthorized", message: "missing dashboard session" } },
        { status: 401 },
      )) as typeof fetch;
    let invalidations = 0;
    const unsubscribe = subscribeDashboardUnauthorized(() => {
      invalidations += 1;
    });

    try {
      await expect(api.me()).rejects.toThrow("missing dashboard session");
      expect(invalidations).toBe(1);
    } finally {
      unsubscribe();
    }
  });

  test("does not invalidate a valid session for rejected login credentials", async () => {
    globalThis.fetch = (async () =>
      Response.json(
        { error: { code: "invalid_credentials", message: "invalid username or password" } },
        { status: 401 },
      )) as typeof fetch;
    let invalidations = 0;
    const unsubscribe = subscribeDashboardUnauthorized(() => {
      invalidations += 1;
    });

    try {
      await expect(api.login("user", "wrong-password", "token")).rejects.toThrow(
        "invalid username or password",
      );
      expect(invalidations).toBe(0);
    } finally {
      unsubscribe();
    }
  });
});
