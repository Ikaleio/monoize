import type { ApiKey } from "@/lib/api";

export type KeyResolutionReason =
  | "ok"
  | "no-keys"
  | "no-model-key"
  | "no-group-key";

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

function allowsModel(key: ApiKey, modelId: string): boolean {
  return (
    !modelId ||
    !key.model_limits_enabled ||
    key.model_limits.length === 0 ||
    key.model_limits.includes(modelId)
  );
}

/**
 * Deterministic key resolution (playground.spec.md PG-AUTH5).
 *
 * Candidate keys must allow the selected model. For a concrete group, candidate
 * tiers are ordered from strictest routing scope to loosest: exact single-group
 * key, multi-group key, then a key that inherits the user's group.
 */
export function resolvePlaygroundKey(
  keys: ApiKey[] | undefined,
  pinnedKeyId: string,
  groupId: string,
  userGroupId: string,
  modelId: string,
): ResolvedPlaygroundKey {
  const timeEligible = (keys ?? []).filter((key) => isEligibleKey(key));
  if (timeEligible.length === 0) {
    return { key: null, reason: "no-keys" };
  }

  const eligible = timeEligible.filter((key) => allowsModel(key, modelId));
  if (eligible.length === 0) {
    return { key: null, reason: "no-model-key" };
  }

  if (!groupId) {
    const pinned = eligible.find((key) => key.id === pinnedKeyId);
    return { key: pinned ?? eligible[0], reason: "ok" };
  }

  const c1 = eligible.filter(
    (key) =>
      !key.use_user_group &&
      key.group_ids.length === 1 &&
      key.group_ids[0] === groupId,
  );
  const c2 = eligible.filter(
    (key) =>
      !key.use_user_group &&
      key.group_ids.length > 1 &&
      key.group_ids.includes(groupId),
  );
  const c3 = eligible.filter(
    (key) =>
      (key.use_user_group || key.group_ids.length === 0) && userGroupId === groupId,
  );

  const covering = [...c1, ...c2, ...c3];
  if (covering.length === 0) {
    return { key: null, reason: "no-group-key" };
  }
  const pinned = covering.find((key) => key.id === pinnedKeyId);
  return { key: pinned ?? covering[0], reason: "ok" };
}
