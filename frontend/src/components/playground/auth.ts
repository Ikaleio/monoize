import type { ApiKey } from "@/lib/api";

export type KeyResolutionReason = "ok" | "no-keys" | "no-group-key";

export interface ResolvedPlaygroundKey {
  key: ApiKey | null;
  reason: KeyResolutionReason;
}

export function isEligibleKey(key: ApiKey, now = Date.now()): boolean {
  if (!key.enabled) return false;
  if (!key.expires_at) return true;
  const expires = Date.parse(key.expires_at);
  return Number.isNaN(expires) || expires > now;
}

/**
 * Deterministic key resolution (playground.spec.md PG-AUTH5).
 *
 * For a concrete group, candidate tiers are ordered from strictest routing scope
 * to loosest: exact single-group key, multi-group key containing the group, then
 * empty-scope key inheriting a user scope that covers the group. An explicitly
 * pinned key wins whenever it appears in any tier.
 */
export function resolvePlaygroundKey(
  keys: ApiKey[] | undefined,
  pinnedKeyId: string,
  group: string,
  userAllowedGroups: string[],
): ResolvedPlaygroundKey {
  const eligible = (keys ?? []).filter((k) => isEligibleKey(k));
  if (eligible.length === 0) {
    return { key: null, reason: "no-keys" };
  }

  if (!group) {
    const pinned = eligible.find((k) => k.id === pinnedKeyId);
    return { key: pinned ?? eligible[0], reason: "ok" };
  }

  const groups = (k: ApiKey) => k.allowed_groups ?? [];
  const c1 = eligible.filter((k) => groups(k).length === 1 && groups(k)[0] === group);
  const c2 = eligible.filter((k) => groups(k).length > 1 && groups(k).includes(group));
  const c3 = eligible.filter(
    (k) =>
      groups(k).length === 0 &&
      (userAllowedGroups.length === 0 || userAllowedGroups.includes(group)),
  );

  const covering = [...c1, ...c2, ...c3];
  if (covering.length === 0) {
    return { key: null, reason: "no-group-key" };
  }
  const pinned = covering.find((k) => k.id === pinnedKeyId);
  return { key: pinned ?? covering[0], reason: "ok" };
}
