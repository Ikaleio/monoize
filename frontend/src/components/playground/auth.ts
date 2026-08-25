import type { ApiKey } from "@/lib/api";

export type KeyResolutionReason =
  | "internal"
  | "ok"
  | "key-unavailable"
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

function coversGroup(
  key: ApiKey,
  groupId: string,
  userGroupId: string,
): boolean {
  if (!groupId) return true;
  if (key.use_user_group || key.group_ids.length === 0) {
    return groupId === userGroupId;
  }
  return key.group_ids.includes(groupId);
}

/**
 * Resolves only the explicitly selected key. An empty selection means the
 * built-in credential; an invalid key never falls back to another user key.
 */
export function resolvePlaygroundKey(
  keys: ApiKey[] | undefined,
  pinnedKeyId: string,
  groupId: string,
  userGroupId: string,
  modelId: string,
): ResolvedPlaygroundKey {
  if (!pinnedKeyId) {
    return { key: null, reason: "internal" };
  }

  const key = (keys ?? []).find(
    (candidate) => candidate.id === pinnedKeyId && isEligibleKey(candidate),
  );
  if (!key) {
    return { key: null, reason: "key-unavailable" };
  }
  if (!allowsModel(key, modelId)) {
    return { key, reason: "no-model-key" };
  }
  if (!coversGroup(key, groupId, userGroupId)) {
    return { key, reason: "no-group-key" };
  }
  return { key, reason: "ok" };
}
