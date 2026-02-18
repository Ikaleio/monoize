import useSWR, { mutate } from "swr";
import type { SWRConfiguration } from "swr";
import { api } from "./api";
import type {
  User,
  ApiKey,
  DashboardStats,
  ConfigOverview,
  SystemSettings,
  PublicSystemSettings,
  Provider,
  CreateProviderInput,
  UpdateProviderInput,
  CreateApiKeyInput,
  UpdateApiKeyInput,
  TransformRegistryItem,
  RequestLogsResponse,
  RequestLogsFilter,
  ModelMetadataRecord,
  UpsertModelMetadataInput,
} from "./api";

// SWR fetcher functions
const fetchers = {
  me: () => api.me(),
  users: () => api.listUsers(),
  apiKeys: () => api.listApiKeys(),
  stats: () => api.getStats(),
  config: () => api.getConfigOverview(),
  settings: () => api.getSettings(),
  publicSettings: () => api.getPublicSettings(),
  providers: () => api.listProviders(),
  transformRegistry: () => api.getTransformRegistry(),
  modelMetadata: () => api.listModelMetadata(),
};

// SWR cache keys
export const SWR_KEYS = {
  ME: "/dashboard/me",
  USERS: "/dashboard/users",
  API_KEYS: "/dashboard/tokens",
  STATS: "/dashboard/stats",
  CONFIG: "/dashboard/config",
  SETTINGS: "/dashboard/settings",
  PUBLIC_SETTINGS: "/dashboard/public-settings",
  PROVIDERS: "/dashboard/providers",
  TRANSFORM_REGISTRY: "/dashboard/transforms/registry",
  MODEL_METADATA: "/dashboard/model-metadata",
  REQUEST_LOGS: "/dashboard/request-logs",
} as const;

// Default SWR config
const defaultConfig: SWRConfiguration = {
  revalidateOnFocus: true,
  revalidateOnReconnect: true,
  dedupingInterval: 2000,
};

// Current user hook
export function useCurrentUser(config?: SWRConfiguration) {
  return useSWR<User>(
    api.getToken() ? SWR_KEYS.ME : null,
    fetchers.me,
    { ...defaultConfig, ...config }
  );
}

// Users list hook (admin only)
export function useUsers(config?: SWRConfiguration) {
  return useSWR<User[]>(SWR_KEYS.USERS, fetchers.users, {
    ...defaultConfig,
    ...config,
  });
}

// API keys hook
export function useApiKeys(config?: SWRConfiguration) {
  return useSWR<ApiKey[]>(SWR_KEYS.API_KEYS, fetchers.apiKeys, {
    ...defaultConfig,
    ...config,
  });
}

// Dashboard stats hook
export function useStats(config?: SWRConfiguration) {
  return useSWR<DashboardStats>(SWR_KEYS.STATS, fetchers.stats, {
    ...defaultConfig,
    ...config,
  });
}

// Config overview hook (admin only)
export function useConfigOverview(config?: SWRConfiguration) {
  return useSWR<ConfigOverview>(SWR_KEYS.CONFIG, fetchers.config, {
    ...defaultConfig,
    ...config,
  });
}

// System settings hook (admin only)
export function useSettings(config?: SWRConfiguration) {
  return useSWR<SystemSettings>(SWR_KEYS.SETTINGS, fetchers.settings, {
    ...defaultConfig,
    ...config,
  });
}

// Public settings hook (no auth required)
export function usePublicSettings(config?: SWRConfiguration) {
  return useSWR<PublicSystemSettings>(
    SWR_KEYS.PUBLIC_SETTINGS,
    fetchers.publicSettings,
    { ...defaultConfig, ...config }
  );
}

// Providers hook (admin only)
export function useProviders(config?: SWRConfiguration) {
  return useSWR<Provider[]>(SWR_KEYS.PROVIDERS, fetchers.providers, {
    ...defaultConfig,
    ...config,
  });
}

export function useTransformRegistry(config?: SWRConfiguration) {
  return useSWR<TransformRegistryItem[]>(
    SWR_KEYS.TRANSFORM_REGISTRY,
    fetchers.transformRegistry,
    {
      ...defaultConfig,
      ...config,
    }
  );
}

export function useModelMetadata(config?: SWRConfiguration) {
  return useSWR<ModelMetadataRecord[]>(
    SWR_KEYS.MODEL_METADATA,
    fetchers.modelMetadata,
    { ...defaultConfig, ...config }
  );
}

export function useRequestLogs(limit = 50, offset = 0, filters?: RequestLogsFilter, config?: SWRConfiguration) {
  const filterKey = filters ? JSON.stringify(filters) : "";
  return useSWR<RequestLogsResponse>(
    `${SWR_KEYS.REQUEST_LOGS}?limit=${limit}&offset=${offset}&f=${filterKey}`,
    () => api.listRequestLogs(limit, offset, filters),
    { ...defaultConfig, ...config }
  );
}

// Mutation helpers with optimistic updates

export async function updateMeOptimistic(
  updates: { email?: string | null },
  currentUser: User | undefined,
  onError?: (error: Error) => void
) {
  if (currentUser) {
    mutate(SWR_KEYS.ME, { ...currentUser, ...updates }, false);
  }

  try {
    const updated = await api.updateMe(updates);
    mutate(SWR_KEYS.ME, updated, false);
    return updated;
  } catch (error) {
    mutate(SWR_KEYS.ME);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateSettingsOptimistic(
  newSettings: SystemSettings,
  onError?: (error: Error) => void
) {
  // Optimistic update
  mutate(SWR_KEYS.SETTINGS, newSettings, false);

  try {
    const updated = await api.updateSettings(newSettings);
    // Revalidate with server data
    mutate(SWR_KEYS.SETTINGS, updated, false);
    return updated;
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.SETTINGS);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function createUserOptimistic(
  username: string,
  password: string,
  role: string,
  currentUsers: User[],
  onError?: (error: Error) => void
) {
  // Optimistic update with placeholder
  const tempUser: User = {
    id: `temp-${Date.now()}`,
    username,
    role: role as User["role"],
    enabled: true,
    created_at: new Date().toISOString(),
    last_login_at: undefined,
    balance_nano_usd: "0",
    balance_usd: "0",
    balance_unlimited: false,
  };
  mutate(SWR_KEYS.USERS, [...currentUsers, tempUser], false);

  try {
    await api.createUser(username, password, role);
    // Revalidate to get the real user data
    mutate(SWR_KEYS.USERS);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.USERS, currentUsers, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateUserOptimistic(
  userId: string,
  updates: Partial<User> & { password?: string },
  currentUsers: User[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const updatedUsers = currentUsers.map((u) =>
    u.id === userId ? { ...u, ...updates } : u
  );
  mutate(SWR_KEYS.USERS, updatedUsers, false);

  try {
    await api.updateUser(userId, updates);
    // Revalidate to get the real data
    mutate(SWR_KEYS.USERS);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.USERS, currentUsers, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteUserOptimistic(
  userId: string,
  currentUsers: User[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const filteredUsers = currentUsers.filter((u) => u.id !== userId);
  mutate(SWR_KEYS.USERS, filteredUsers, false);

  try {
    await api.deleteUser(userId);
    // Revalidate
    mutate(SWR_KEYS.USERS);
    mutate(SWR_KEYS.STATS);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.USERS, currentUsers, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function createApiKeyOptimistic(
  input: CreateApiKeyInput,
  _currentKeys: ApiKey[],
  onError?: (error: Error) => void
) {
  try {
    const result = await api.createApiKey(input);
    // Revalidate to get the new key in list
    mutate(SWR_KEYS.API_KEYS);
    mutate(SWR_KEYS.STATS);
    return result;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateApiKeyOptimistic(
  keyId: string,
  input: UpdateApiKeyInput,
  currentKeys: ApiKey[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const updatedKeys = currentKeys.map((k) =>
    k.id === keyId ? { ...k, ...input } : k
  );
  mutate(SWR_KEYS.API_KEYS, updatedKeys, false);

  try {
    const result = await api.updateApiKey(keyId, input);
    // Revalidate
    mutate(SWR_KEYS.API_KEYS);
    return result;
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.API_KEYS, currentKeys, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteApiKeyOptimistic(
  keyId: string,
  currentKeys: ApiKey[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const filteredKeys = currentKeys.filter((k) => k.id !== keyId);
  mutate(SWR_KEYS.API_KEYS, filteredKeys, false);

  try {
    await api.deleteApiKey(keyId);
    // Revalidate
    mutate(SWR_KEYS.API_KEYS);
    mutate(SWR_KEYS.STATS);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.API_KEYS, currentKeys, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function batchDeleteApiKeysOptimistic(
  keyIds: string[],
  currentKeys: ApiKey[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const filteredKeys = currentKeys.filter((k) => !keyIds.includes(k.id));
  mutate(SWR_KEYS.API_KEYS, filteredKeys, false);

  try {
    await api.batchDeleteApiKeys(keyIds);
    // Revalidate
    mutate(SWR_KEYS.API_KEYS);
    mutate(SWR_KEYS.STATS);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.API_KEYS, currentKeys, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Provider mutation helpers
export async function createProviderOptimistic(
  input: CreateProviderInput,
  _currentProviders: Provider[],
  onError?: (error: Error) => void
) {
  try {
    const result = await api.createProvider(input);
    // Revalidate to get the new provider
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.STATS);
    mutate(SWR_KEYS.CONFIG);
    return result;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateProviderOptimistic(
  id: string,
  input: UpdateProviderInput,
  currentProviders: Provider[],
  onError?: (error: Error) => void
) {
  const updatedProviders = currentProviders.map((p) =>
    p.id === id ? { ...p, ...input } : p
  );
  mutate(SWR_KEYS.PROVIDERS, updatedProviders, false);

  try {
    const result = await api.updateProvider(id, input);
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.CONFIG);
    return result;
  } catch (error) {
    // Revalidate from server rather than rolling back to a potentially stale snapshot
    mutate(SWR_KEYS.PROVIDERS);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteProviderOptimistic(
  id: string,
  currentProviders: Provider[],
  onError?: (error: Error) => void
) {
  // Optimistic update
  const filteredProviders = currentProviders.filter((p) => p.id !== id);
  mutate(SWR_KEYS.PROVIDERS, filteredProviders, false);

  try {
    await api.deleteProvider(id);
    // Revalidate
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.STATS);
    mutate(SWR_KEYS.CONFIG);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.PROVIDERS, currentProviders, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function reorderProviders(
  providerIds: string[],
  onError?: (error: Error) => void
) {
  try {
    await api.reorderProviders(providerIds);
    mutate(SWR_KEYS.PROVIDERS);
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function upsertModelMetadataOptimistic(
  modelId: string,
  input: UpsertModelMetadataInput,
  currentRecords: ModelMetadataRecord[],
  onError?: (error: Error) => void
) {
  const tempRecord: ModelMetadataRecord = {
    model_id: modelId,
    source: "manual",
    updated_at: new Date().toISOString(),
    raw_json: {},
    ...input,
    models_dev_provider: input.models_dev_provider ?? undefined,
    mode: input.mode ?? undefined,
    input_cost_per_token_nano: input.input_cost_per_token_nano ?? undefined,
    output_cost_per_token_nano: input.output_cost_per_token_nano ?? undefined,
    cache_read_input_cost_per_token_nano: input.cache_read_input_cost_per_token_nano ?? undefined,
    output_cost_per_reasoning_token_nano: input.output_cost_per_reasoning_token_nano ?? undefined,
    max_input_tokens: input.max_input_tokens ?? undefined,
    max_output_tokens: input.max_output_tokens ?? undefined,
    max_tokens: input.max_tokens ?? undefined,
  };
  const exists = currentRecords.some((r) => r.model_id === modelId);
  const optimistic = exists
    ? currentRecords.map((r) => (r.model_id === modelId ? { ...r, ...tempRecord } : r))
    : [...currentRecords, tempRecord];
  mutate(SWR_KEYS.MODEL_METADATA, optimistic, false);

  try {
    const result = await api.upsertModelMetadata(modelId, input);
    mutate(SWR_KEYS.MODEL_METADATA);
    return result;
  } catch (error) {
    mutate(SWR_KEYS.MODEL_METADATA, currentRecords, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteModelMetadataOptimistic(
  modelId: string,
  currentRecords: ModelMetadataRecord[],
  onError?: (error: Error) => void
) {
  const filtered = currentRecords.filter((r) => r.model_id !== modelId);
  mutate(SWR_KEYS.MODEL_METADATA, filtered, false);

  try {
    await api.deleteModelMetadata(modelId);
    mutate(SWR_KEYS.MODEL_METADATA);
  } catch (error) {
    mutate(SWR_KEYS.MODEL_METADATA, currentRecords, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function syncModelMetadata(
  onError?: (error: Error) => void
) {
  try {
    const result = await api.syncModelMetadataFromModelsDev();
    mutate(SWR_KEYS.MODEL_METADATA);
    return result;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Global revalidation helpers
export function revalidateAll() {
  mutate(SWR_KEYS.ME);
  mutate(SWR_KEYS.USERS);
  mutate(SWR_KEYS.API_KEYS);
  mutate(SWR_KEYS.STATS);
  mutate(SWR_KEYS.CONFIG);
  mutate(SWR_KEYS.SETTINGS);
  mutate(SWR_KEYS.TRANSFORM_REGISTRY);
}

export function clearCache() {
  mutate(SWR_KEYS.ME, undefined, false);
  mutate(SWR_KEYS.USERS, undefined, false);
  mutate(SWR_KEYS.API_KEYS, undefined, false);
  mutate(SWR_KEYS.STATS, undefined, false);
  mutate(SWR_KEYS.CONFIG, undefined, false);
  mutate(SWR_KEYS.SETTINGS, undefined, false);
  mutate(SWR_KEYS.TRANSFORM_REGISTRY, undefined, false);
}
