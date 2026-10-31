import useSWR, { mutate } from "swr";
import type { SWRConfiguration } from "swr";
import { api } from "./api";
import type { SpendWindow } from "./spend-window";
import type {
  User,
  ApiKey,
  DashboardStats,
  DashboardAnalytics,
  AdminOverview,
  DashboardPerformance,
  ConfigOverview,
  SystemSettings,
  PublicSystemSettings,
  Provider,
  CreateProviderInput,
  UpdateProviderInput,
  CreateApiKeyInput,
  UpdateApiKeyInput,
  UpdateUserInput,
  TransformRegistryItem,
  RequestLogsResponse,
  RequestLogsFilter,
  RequestCaptureDetail,
  ModelMetadataRecord,
  MarketplaceModelRecord,
  UpsertModelMetadataInput,
  ModelPriceRecord,
  UpsertModelPriceInput,
  PriceSyncRun,
  PriceSyncSource,
  UnpricedModels,
  BillingPlan,
  BillingPlanInput,
  BillingPlanSubscription,
  Group,
  CreateGroupInput,
  UpdateGroupInput,
  UserLiveUsage,
  CustomTransform,
  CustomTransformReferences,
  RechargeChannel,
  RechargeOrder,
  RechargeOrdersResponse,
  RechargeOrdersFilter,
  CreateRechargeOrderInput,
  CreateRechargeOrderResponse,
  LedgerResponse,
  PaymentChannel,
  CreatePaymentChannelInput,
  UpdatePaymentChannelInput,
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
  dashboardGroups: async () => (await api.listDashboardGroups()).groups,
  transformRegistry: () => api.getTransformRegistry(),
  modelMetadata: () => api.listModelMetadata(),
  modelPrices: () => api.listModelPrices(),
  unpricedModels: () => api.listUnpricedModels(),
  priceSyncRuns: () => api.listPriceSyncRuns(),
  billingPlans: () => api.listBillingPlans(),
  billingPlanMarketplace: () => api.listBillingPlanMarketplace(),
  billingPlanSubscription: () => api.getBillingPlanSubscription(),
  marketplaceModels: () => api.listMarketplaceModels(),
  customTransforms: () => api.listCustomTransforms(),
  customTransformReferences: () => api.listCustomTransformReferences(),
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
  DASHBOARD_GROUPS: "/dashboard/groups",
  TRANSFORM_REGISTRY: "/dashboard/transforms/registry",
  MODEL_METADATA: "/dashboard/model-metadata",
  MODEL_PRICES: "/dashboard/model-prices",
  UNPRICED_MODELS: "/dashboard/model-prices/unpriced",
  PRICE_SYNC_RUNS: "/dashboard/price-sync/runs",
  MARKETPLACE_MODELS: "/dashboard/marketplace/models",
  BILLING_PLANS: "/dashboard/billing-plans",
  BILLING_PLAN_MARKETPLACE: "/dashboard/billing-plans/marketplace",
  BILLING_PLAN_SUBSCRIPTION: "/dashboard/billing-plan-subscription",
  REQUEST_LOGS: "/dashboard/request-logs",
  ANALYTICS: "/dashboard/analytics",
  PERFORMANCE: "/dashboard/performance",
  ADMIN_OVERVIEW: "/dashboard/admin/overview",
  LIVE_USAGE: "/dashboard/me/live-usage",
  CUSTOM_TRANSFORMS: "/dashboard/custom-transforms",
  CUSTOM_TRANSFORM_REFERENCES: "/dashboard/custom-transforms/references",
  RECHARGE_CHANNELS: "/dashboard/recharge/channels",
  RECHARGE_ORDERS: "/dashboard/recharge/orders",
  LEDGER: "/dashboard/ledger",
  PAYMENT_CHANNELS: "/dashboard/payment-channels",
} as const;

export function providerDetailSWRKey(providerId: string) {
  return `provider-detail:${providerId}`;
}

export function rechargeOrdersSWRKey(
  limit: number,
  offset: number,
  filters?: RechargeOrdersFilter,
) {
  const filterKey = filters ? JSON.stringify(filters) : "";
  return `${SWR_KEYS.RECHARGE_ORDERS}?limit=${limit}&offset=${offset}&f=${filterKey}`;
}

export function ledgerSWRKey(
  limit: number,
  offset: number,
  kinds?: string[],
  username?: string,
) {
  return `${SWR_KEYS.LEDGER}?limit=${limit}&offset=${offset}&kinds=${kinds?.join(",") ?? ""}&u=${username ?? ""}`;
}

export function userBillingPlanSubscriptionSWRKey(userId: string) {
  return `${SWR_KEYS.USERS}/${userId}/billing-plan-subscription`;
}

// Default SWR config
const defaultConfig: SWRConfiguration = {
  revalidateOnFocus: true,
  revalidateOnReconnect: true,
  dedupingInterval: 2000,
};

// Current user hook
export function useCurrentUser(config?: SWRConfiguration) {
  return useSWR<User>(SWR_KEYS.ME, fetchers.me, {
    ...defaultConfig,
    ...config,
  });
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

/** Admin read-only keys of another user (TM 2.1a); `null` pauses the request. */
export function useUserApiKeys(userId: string | null, config?: SWRConfiguration) {
  return useSWR<ApiKey[]>(
    userId ? `${SWR_KEYS.USERS}/${userId}/api-keys` : null,
    () => api.listUserApiKeys(userId!),
    { ...defaultConfig, ...config },
  );
}

// Billing plans hook (admin only)
export function useBillingPlans(config?: SWRConfiguration) {
  return useSWR<BillingPlan[]>(SWR_KEYS.BILLING_PLANS, fetchers.billingPlans, {
    ...defaultConfig,
    ...config,
  });
}

export function useBillingPlanMarketplace(config?: SWRConfiguration) {
  return useSWR<BillingPlan[]>(
    SWR_KEYS.BILLING_PLAN_MARKETPLACE,
    fetchers.billingPlanMarketplace,
    { ...defaultConfig, ...config },
  );
}

export function useBillingPlanSubscription(config?: SWRConfiguration) {
  return useSWR<BillingPlanSubscription | null>(
    SWR_KEYS.BILLING_PLAN_SUBSCRIPTION,
    fetchers.billingPlanSubscription,
    { ...defaultConfig, ...config },
  );
}

export function useUserBillingPlanSubscription(
  userId: string | null,
  config?: SWRConfiguration,
) {
  return useSWR<BillingPlanSubscription | null>(
    userId ? userBillingPlanSubscriptionSWRKey(userId) : null,
    () => api.getUserBillingPlanSubscription(userId!),
    {
      ...defaultConfig,
      ...config,
    },
  );
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
    { ...defaultConfig, ...config },
  );
}

// Providers hook (admin only)
export function useProviders(config?: SWRConfiguration, enabled = true) {
  return useSWR<Provider[]>(
    enabled ? SWR_KEYS.PROVIDERS : null,
    fetchers.providers,
    {
      ...defaultConfig,
      ...config,
    },
  );
}

export function useProviderDetail(
  providerId: string | null | undefined,
  config?: SWRConfiguration,
) {
  return useSWR<Provider>(
    providerId ? providerDetailSWRKey(providerId) : null,
    () => api.getProvider(providerId!),
    { ...defaultConfig, ...config },
  );
}

export function useDashboardGroups(enabled = true, config?: SWRConfiguration) {
  return useSWR<Group[]>(
    enabled ? SWR_KEYS.DASHBOARD_GROUPS : null,
    fetchers.dashboardGroups,
    {
      ...defaultConfig,
      ...config,
    },
  );
}

export function useTransformRegistry(config?: SWRConfiguration) {
  return useSWR<TransformRegistryItem[]>(
    SWR_KEYS.TRANSFORM_REGISTRY,
    fetchers.transformRegistry,
    {
      ...defaultConfig,
      ...config,
    },
  );
}

// Custom transforms hook (admin only; custom-js-transforms.spec.md CJS-UI-2)
export function useCustomTransforms(config?: SWRConfiguration) {
  return useSWR<CustomTransform[]>(
    SWR_KEYS.CUSTOM_TRANSFORMS,
    fetchers.customTransforms,
    { ...defaultConfig, ...config },
  );
}

// Chains referencing each custom transform (admin only; CJS-UI-8)
export function useCustomTransformReferences(config?: SWRConfiguration) {
  return useSWR<CustomTransformReferences>(
    SWR_KEYS.CUSTOM_TRANSFORM_REFERENCES,
    fetchers.customTransformReferences,
    { ...defaultConfig, ...config },
  );
}

export function useModelMetadata(config?: SWRConfiguration) {
  return useSWR<ModelMetadataRecord[]>(
    SWR_KEYS.MODEL_METADATA,
    fetchers.modelMetadata,
    { ...defaultConfig, ...config },
  );
}

export function useModelPrices(config?: SWRConfiguration) {
  return useSWR<ModelPriceRecord[]>(
    SWR_KEYS.MODEL_PRICES,
    fetchers.modelPrices,
    {
      ...defaultConfig,
      ...config,
    },
  );
}

export function useUnpricedModels(config?: SWRConfiguration) {
  return useSWR<UnpricedModels>(SWR_KEYS.UNPRICED_MODELS, fetchers.unpricedModels, {
    ...defaultConfig,
    ...config,
  });
}

export function usePriceSyncRuns(config?: SWRConfiguration) {
  return useSWR<PriceSyncRun[]>(
    SWR_KEYS.PRICE_SYNC_RUNS,
    fetchers.priceSyncRuns,
    {
      ...defaultConfig,
      ...config,
    },
  );
}

export function useMarketplaceModels(config?: SWRConfiguration) {
  return useSWR<MarketplaceModelRecord[]>(
    SWR_KEYS.MARKETPLACE_MODELS,
    fetchers.marketplaceModels,
    { ...defaultConfig, ...config },
  );
}

export function useRequestLogs(
  limit = 50,
  offset = 0,
  filters?: RequestLogsFilter,
  config?: SWRConfiguration,
) {
  const filterKey = filters ? JSON.stringify(filters) : "";
  return useSWR<RequestLogsResponse>(
    `${SWR_KEYS.REQUEST_LOGS}?limit=${limit}&offset=${offset}&f=${filterKey}`,
    () => api.listRequestLogs(limit, offset, filters),
    { ...defaultConfig, ...config },
  );
}

// Capture detail hook (request-capture-viewer.spec.md RCV-F5): the key is
// null until `enabled` is true, so no fetch happens before the dialog opens;
// the cache entry survives close/reopen per normal SWR semantics.
export function useRequestCapture(
  requestId: string | null | undefined,
  userId: string | null | undefined,
  enabled: boolean,
  config?: SWRConfiguration,
) {
  return useSWR<RequestCaptureDetail>(
    enabled && requestId
      ? `${SWR_KEYS.REQUEST_LOGS}/capture?rid=${encodeURIComponent(requestId)}&uid=${encodeURIComponent(userId ?? "")}`
      : null,
    () => api.getRequestCapture(requestId as string, userId ?? undefined),
    { ...defaultConfig, revalidateOnFocus: false, ...config },
  );
}

export function useDashboardAnalytics(
  buckets = 8,
  rangeHours = 24,
  config?: SWRConfiguration,
) {
  const tzOffsetMinutes = -new Date().getTimezoneOffset();
  return useSWR<DashboardAnalytics>(
    `${SWR_KEYS.ANALYTICS}?buckets=${buckets}&range_hours=${rangeHours}&tz=${tzOffsetMinutes}`,
    () => api.getDashboardAnalytics(buckets, rangeHours, tzOffsetMinutes),
    { ...defaultConfig, ...config },
  );
}

export function useDashboardPerformance(config?: SWRConfiguration) {
  return useSWR<DashboardPerformance>(
    SWR_KEYS.PERFORMANCE,
    () => api.getDashboardPerformance(),
    { ...defaultConfig, refreshInterval: 60000, ...config },
  );
}

// Own rolling 60-second usage hook (user-live-usage.spec.md LU-9/LU-10).
// Mounted only while the user-center dropdown is open, so the 10s poll
// runs only while the menu is visible.
export function useLiveUsage(config?: SWRConfiguration) {
  return useSWR<UserLiveUsage>(
    SWR_KEYS.LIVE_USAGE,
    () => api.getMyLiveUsage(),
    {
      ...defaultConfig,
      refreshInterval: 10000,
      ...config,
    },
  );
}

// Recharge hooks (recharge-system.spec.md §9-§11)

export function useRechargeChannels(config?: SWRConfiguration) {
  return useSWR<RechargeChannel[]>(
    SWR_KEYS.RECHARGE_CHANNELS,
    () => api.listRechargeChannels(),
    { ...defaultConfig, ...config },
  );
}

// RC-W4/RC-M3 order list. keepPreviousData avoids skeleton flashes while the
// pending-order poll (refreshInterval set by the caller) revalidates the page.
export function useRechargeOrders(
  limit = 20,
  offset = 0,
  filters?: RechargeOrdersFilter,
  config?: SWRConfiguration,
) {
  return useSWR<RechargeOrdersResponse>(
    rechargeOrdersSWRKey(limit, offset, filters),
    () => api.listRechargeOrders(limit, offset, filters),
    {
      ...defaultConfig,
      keepPreviousData: true,
      ...config,
    },
  );
}

export function useLedger(
  limit = 20,
  offset = 0,
  kinds?: string[],
  username?: string,
  config?: SWRConfiguration,
) {
  return useSWR<LedgerResponse>(
    ledgerSWRKey(limit, offset, kinds, username),
    () => api.listLedger(limit, offset, kinds, username),
    {
      ...defaultConfig,
      keepPreviousData: true,
      ...config,
    },
  );
}

// Admin payment channels (§9.2)
export function usePaymentChannels(config?: SWRConfiguration) {
  return useSWR<PaymentChannel[]>(
    SWR_KEYS.PAYMENT_CHANNELS,
    () => api.listPaymentChannels(),
    { ...defaultConfig, ...config },
  );
}

// Admin overview hook (admin dashboard; AD-ADF-7: 10s refresh, skeleton, retry)
export function useAdminOverview(
  spendWindow: SpendWindow,
  config?: SWRConfiguration,
) {
  return useSWR<AdminOverview>(
    `${SWR_KEYS.ADMIN_OVERVIEW}?spend_window=${spendWindow}`,
    () => api.getAdminOverview(spendWindow),
    {
      ...defaultConfig,
      refreshInterval: 10000,
      keepPreviousData: true,
      ...config,
    },
  );
}

// Mutation helpers with optimistic updates

export async function updateMeOptimistic(
  updates: { email?: string | null },
  currentUser: User | undefined,
  onError?: (error: Error) => void,
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

/**
 * System settings page write (system-settings-ui.spec.md SSU-22): sends only the changed
 * keys, then refreshes the public settings and Providers that derive from them.
 */
export async function patchSettingsOptimistic(patch: Partial<SystemSettings>) {
  const updated = await patchPricingSettingsOptimistic(patch);
  mutate(SWR_KEYS.PUBLIC_SETTINGS);
  mutate(SWR_KEYS.PROVIDERS);
  return updated;
}

export async function createUserOptimistic(
  username: string,
  password: string,
  role: string,
  groupId: string | undefined,
  currentUsers: User[],
  onError?: (error: Error) => void,
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
    group_id: groupId ?? "",
    today_calls: 0,
    today_cost_nano_usd: "0",
    today_cost_usd: "0",
  };
  mutate(SWR_KEYS.USERS, [...currentUsers, tempUser], false);

  try {
    await api.createUser(username, password, role, groupId);
    // Revalidate to get the real user data
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

export async function updateUserOptimistic(
  userId: string,
  updates: UpdateUserInput,
  currentUsers: User[],
  onError?: (error: Error) => void,
) {
  // Only fields that map 1:1 onto the list row are applied optimistically;
  // delta/expected/note/password are request-only and the response fills in the rest.
  const optimisticFields: Partial<User> = {};
  if (updates.username !== undefined) optimisticFields.username = updates.username;
  if (updates.role !== undefined) optimisticFields.role = updates.role;
  if (updates.enabled !== undefined) optimisticFields.enabled = updates.enabled;
  if (updates.balance_unlimited !== undefined) {
    optimisticFields.balance_unlimited = updates.balance_unlimited;
  }
  if (updates.email !== undefined) optimisticFields.email = updates.email;
  if (updates.group_id !== undefined) optimisticFields.group_id = updates.group_id;
  const updatedUsers = currentUsers.map((u) =>
    u.id === userId ? { ...u, ...optimisticFields } : u,
  );
  mutate(SWR_KEYS.USERS, updatedUsers, false);

  try {
    const updated = await api.updateUser(userId, updates);
    mutate(
      SWR_KEYS.USERS,
      (users: User[] | undefined) =>
        users?.map((u) => (u.id === userId ? { ...u, ...updated } : u)),
      false,
    );
    // Revalidate to get the real data
    mutate(SWR_KEYS.USERS);
    mutate(SWR_KEYS.ME);
    mutate(SWR_KEYS.STATS);
    return updated;
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
  onError?: (error: Error) => void,
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

// Group registry mutation helpers

function sortGroups(groups: Group[]): Group[] {
  return [...groups].sort(
    (a, b) =>
      a.sort_order - b.sort_order ||
      a.created_at.localeCompare(b.created_at) ||
      a.id.localeCompare(b.id),
  );
}

export async function createGroupOptimistic(
  input: CreateGroupInput,
  currentGroups: Group[],
  onError?: (error: Error) => void,
) {
  const now = new Date().toISOString();
  const tempGroup: Group = {
    id: `temp-${Date.now()}`,
    name: input.name.trim(),
    description: (input.description ?? "").trim(),
    is_default: false,
    user_selectable: input.user_selectable ?? false,
    // GR-A3a: an omitted sort_order appends after the current last group.
    sort_order:
      input.sort_order ??
      currentGroups.reduce((max, group) => Math.max(max, group.sort_order), -1) + 1,
    billing_ratio: input.billing_ratio ?? "1",
    created_at: now,
    updated_at: now,
  };
  mutate(
    SWR_KEYS.DASHBOARD_GROUPS,
    sortGroups([...currentGroups, tempGroup]),
    false,
  );

  try {
    const created = await api.createGroup(input);
    mutate(SWR_KEYS.DASHBOARD_GROUPS);
    return created;
  } catch (error) {
    mutate(SWR_KEYS.DASHBOARD_GROUPS, currentGroups, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateGroupOptimistic(
  groupId: string,
  input: UpdateGroupInput,
  currentGroups: Group[],
  onError?: (error: Error) => void,
) {
  const optimistic = sortGroups(
    currentGroups.map((g) =>
      g.id === groupId
        ? {
            ...g,
            name: input.name ?? g.name,
            description: input.description ?? g.description,
            user_selectable: input.user_selectable ?? g.user_selectable,
            sort_order: input.sort_order ?? g.sort_order,
            billing_ratio: input.billing_ratio ?? g.billing_ratio,
            updated_at: new Date().toISOString(),
          }
        : g,
    ),
  );
  mutate(SWR_KEYS.DASHBOARD_GROUPS, optimistic, false);

  try {
    const updated = await api.updateGroup(groupId, input);
    mutate(SWR_KEYS.DASHBOARD_GROUPS);
    return updated;
  } catch (error) {
    mutate(SWR_KEYS.DASHBOARD_GROUPS, currentGroups, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function reorderGroupsOptimistic(
  groupIds: string[],
  currentGroups: Group[],
  onError?: (error: Error) => void,
) {
  const groupsById = new Map(currentGroups.map((group) => [group.id, group]));
  const updatedAt = new Date().toISOString();
  const optimistic = groupIds.flatMap((groupId, sortOrder) => {
    const group = groupsById.get(groupId);
    return group
      ? [{ ...group, sort_order: sortOrder, updated_at: updatedAt }]
      : [];
  });
  mutate(SWR_KEYS.DASHBOARD_GROUPS, optimistic, false);

  try {
    await api.reorderGroups(groupIds);
    mutate(SWR_KEYS.DASHBOARD_GROUPS);
  } catch (error) {
    mutate(SWR_KEYS.DASHBOARD_GROUPS, currentGroups, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteGroupOptimistic(
  groupId: string,
  currentGroups: Group[],
  onError?: (error: Error) => void,
) {
  mutate(
    SWR_KEYS.DASHBOARD_GROUPS,
    currentGroups.filter((g) => g.id !== groupId),
    false,
  );

  try {
    await api.deleteGroup(groupId);
    // Deletion cascades to users, API keys, providers, and billing plans.
    mutate(SWR_KEYS.DASHBOARD_GROUPS);
    mutate(SWR_KEYS.USERS);
    mutate(SWR_KEYS.API_KEYS);
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.BILLING_PLANS);
    mutate(SWR_KEYS.ME);
  } catch (error) {
    mutate(SWR_KEYS.DASHBOARD_GROUPS, currentGroups, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function createBillingPlanOptimistic(
  input: BillingPlanInput,
  currentPlans: BillingPlan[],
  onError?: (error: Error) => void,
) {
  const now = new Date().toISOString();
  const tempPlan: BillingPlan = {
    id: `temp-${Date.now()}`,
    ...input,
    prices: input.prices.map((price, index) => ({
      id: `temp-price-${index}`,
      price_nano_usd: price.price_nano_usd ?? "0",
      price_usd: price.price_usd ?? "0",
      duration_seconds: price.duration_seconds,
      created_at: now,
    })),
    created_at: now,
    updated_at: now,
  };
  mutate(SWR_KEYS.BILLING_PLANS, [...currentPlans, tempPlan], false);

  try {
    await api.createBillingPlan(input);
    mutate(SWR_KEYS.BILLING_PLANS);
    mutate(SWR_KEYS.BILLING_PLAN_MARKETPLACE);
  } catch (error) {
    mutate(SWR_KEYS.BILLING_PLANS, currentPlans, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateBillingPlanOptimistic(
  planId: string,
  input: BillingPlanInput,
  currentPlans: BillingPlan[],
  onError?: (error: Error) => void,
) {
  const updatedPlans = currentPlans.map((p) =>
    p.id === planId
      ? {
          ...p,
          ...input,
          prices: input.prices.map((price, index) => ({
            id: p.prices[index]?.id ?? `temp-price-${index}`,
            price_nano_usd: price.price_nano_usd ?? "0",
            price_usd: price.price_usd ?? "0",
            duration_seconds: price.duration_seconds,
            created_at: p.prices[index]?.created_at ?? new Date().toISOString(),
          })),
          updated_at: new Date().toISOString(),
        }
      : p,
  );
  mutate(SWR_KEYS.BILLING_PLANS, updatedPlans, false);

  try {
    await api.updateBillingPlan(planId, input);
    mutate(SWR_KEYS.BILLING_PLANS);
    mutate(SWR_KEYS.BILLING_PLAN_MARKETPLACE);
  } catch (error) {
    mutate(SWR_KEYS.BILLING_PLANS, currentPlans, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteBillingPlanOptimistic(
  planId: string,
  currentPlans: BillingPlan[],
  onError?: (error: Error) => void,
) {
  const filteredPlans = currentPlans.filter((p) => p.id !== planId);
  mutate(SWR_KEYS.BILLING_PLANS, filteredPlans, false);

  try {
    await api.deleteBillingPlan(planId);
    mutate(SWR_KEYS.BILLING_PLANS);
    mutate(SWR_KEYS.BILLING_PLAN_MARKETPLACE);
  } catch (error) {
    mutate(SWR_KEYS.BILLING_PLANS, currentPlans, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function purchaseBillingPlanOptimistic(
  planId: string,
  priceId: string,
  priceNanoUsd: string,
  onError?: (error: Error) => void,
) {
  let userSnapshot: User | undefined;
  await mutate(
    SWR_KEYS.ME,
    (current: User | undefined) => {
      userSnapshot = current;
      if (!current || current.balance_unlimited) return current;
      try {
        const next = BigInt(current.balance_nano_usd) - BigInt(priceNanoUsd);
        if (next < 0n) return current;
        const fraction = (next % 1_000_000_000n)
          .toString()
          .padStart(9, "0")
          .replace(/0+$/, "");
        return {
          ...current,
          balance_nano_usd: next.toString(),
          balance_usd: `${next / 1_000_000_000n}${fraction ? `.${fraction}` : ""}`,
        };
      } catch {
        return current;
      }
    },
    false,
  );
  try {
    const subscription = await api.purchaseBillingPlan(planId, priceId);
    mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, subscription, false);
    mutate(SWR_KEYS.BILLING_PLAN_MARKETPLACE);
    revalidateRechargeCaches();
    return subscription;
  } catch (error) {
    mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION);
    mutate(SWR_KEYS.ME, userSnapshot, false);
    if (onError && error instanceof Error) onError(error);
    throw error;
  }
}

function emptyWindowUsage(limit: string | null) {
  return limit
    ? {
        limit_nano_usd: limit,
        used_nano_usd: "0",
        remaining_nano_usd: limit,
        next_reset_at: null,
      }
    : null;
}

export async function assignUserBillingPlanSubscriptionOptimistic(
  userId: string,
  plan: BillingPlan,
  durationSeconds: number,
  currentSubscription: BillingPlanSubscription | null | undefined,
  isCurrentUser: boolean,
  onError?: (error: Error) => void,
) {
  const startsAt = new Date();
  const optimistic: BillingPlanSubscription = {
    id: `temp-${Date.now()}`,
    user_id: userId,
    plan_id: plan.id,
    price_id: null,
    plan_name: plan.name,
    plan_description: plan.description,
    group_ids: plan.group_ids,
    multiplier: plan.multiplier,
    price_nano_usd: null,
    starts_at: startsAt.toISOString(),
    expires_at: new Date(
      startsAt.getTime() + durationSeconds * 1000,
    ).toISOString(),
    windows: {
      five_hour: emptyWindowUsage(plan.limit_5h_nano_usd),
      twenty_four_hour: emptyWindowUsage(plan.limit_24h_nano_usd),
      seven_day: emptyWindowUsage(plan.limit_7d_nano_usd),
      thirty_day: emptyWindowUsage(plan.limit_30d_nano_usd),
    },
  };
  const key = userBillingPlanSubscriptionSWRKey(userId);
  mutate(key, optimistic, false);
  if (isCurrentUser)
    mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, optimistic, false);
  try {
    const subscription = await api.assignUserBillingPlanSubscription(
      userId,
      plan.id,
      durationSeconds,
    );
    mutate(key, subscription, false);
    if (isCurrentUser) {
      mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, subscription, false);
    }
    void mutate(key);
    if (isCurrentUser) void mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION);
    return subscription;
  } catch (error) {
    mutate(key, currentSubscription, false);
    if (isCurrentUser) {
      mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, currentSubscription, false);
    }
    if (onError && error instanceof Error) onError(error);
    throw error;
  }
}

export async function revokeUserBillingPlanSubscriptionOptimistic(
  userId: string,
  currentSubscription: BillingPlanSubscription | null | undefined,
  isCurrentUser: boolean,
  onError?: (error: Error) => void,
) {
  const key = userBillingPlanSubscriptionSWRKey(userId);
  mutate(key, null, false);
  if (isCurrentUser) mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, null, false);
  try {
    await api.revokeUserBillingPlanSubscription(userId);
    mutate(key);
    if (isCurrentUser) mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION);
  } catch (error) {
    mutate(key, currentSubscription, false);
    if (isCurrentUser) {
      mutate(SWR_KEYS.BILLING_PLAN_SUBSCRIPTION, currentSubscription, false);
    }
    if (onError && error instanceof Error) onError(error);
    throw error;
  }
}

export async function createApiKeyOptimistic(
  input: CreateApiKeyInput,
  _currentKeys: ApiKey[],
  onError?: (error: Error) => void,
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
  onError?: (error: Error) => void,
) {
  // Optimistic update
  const updatedKeys = currentKeys.map((k) =>
    k.id === keyId ? { ...k, ...input } : k,
  );
  mutate(SWR_KEYS.API_KEYS, updatedKeys, false);

  try {
    const result = await api.updateApiKey(keyId, input);
    mutate(
      SWR_KEYS.API_KEYS,
      (keys: ApiKey[] | undefined) => keys?.map((k) => (k.id === keyId ? result : k)),
      false,
    );
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

/** SA-TX: moves `amountUsd` from the owner's wallet into the key's sub-account (TM-UI6). */
export async function transferToSubAccountAndRefresh(keyId: string, amountUsd: string) {
  const result = await api.transferToSubAccount(keyId, { amount_usd: amountUsd });
  mutate(
    SWR_KEYS.API_KEYS,
    (keys: ApiKey[] | undefined) =>
      keys?.map((k) =>
        k.id === keyId
          ? {
              ...k,
              sub_account_balance_nano_usd: result.api_key_balance_nano_usd,
              sub_account_balance_usd: nanoToUsdDecimalString(result.api_key_balance_nano_usd),
            }
          : k,
      ),
    false,
  );
  mutate(SWR_KEYS.API_KEYS);
  return result;
}

function nanoToUsdDecimalString(nano: string): string {
  const value = BigInt(nano);
  const negative = value < 0n;
  const absolute = negative ? -value : value;
  const fraction = (absolute % 1_000_000_000n).toString().padStart(9, "0").replace(/0+$/, "");
  const whole = (absolute / 1_000_000_000n).toString();
  return `${negative ? "-" : ""}${whole}${fraction ? `.${fraction}` : ""}`;
}

export async function deleteApiKeyOptimistic(
  keyId: string,
  currentKeys: ApiKey[],
  onError?: (error: Error) => void,
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
  onError?: (error: Error) => void,
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

/** Builds the list entry shown while a create request is pending. */
function pendingProvider(input: CreateProviderInput, currentProviders: Provider[]): Provider {
  const now = new Date().toISOString();
  const id = `pending-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return {
    id,
    name: input.name,
    channels: input.channels.map((channel, index) => {
      const entry = {
        ...channel,
        id: channel.id ?? `${id}-${index}`,
        weight: channel.weight ?? 1,
        enabled: channel.enabled ?? true,
      };
      delete entry.api_key;
      return entry;
    }),
    max_retries: input.max_retries ?? -1,
    channel_max_retries: input.channel_max_retries ?? 0,
    channel_retry_interval_ms: input.channel_retry_interval_ms ?? 0,
    circuit_breaker_enabled: input.circuit_breaker_enabled ?? true,
    per_model_circuit_break: input.per_model_circuit_break ?? false,
    transforms: input.transforms ?? [],
    api_type_overrides: input.api_type_overrides ?? [],
    group_ids: input.group_ids ?? [],
    enabled: input.enabled ?? true,
    // The server appends a provider without an explicit priority after the current maximum.
    priority:
      input.priority ??
      currentProviders.reduce((max, provider) => Math.max(max, provider.priority + 1), 0),
    created_at: now,
    updated_at: now,
  };
}

export async function createProviderOptimistic(
  input: CreateProviderInput,
  currentProviders: Provider[],
  onError?: (error: Error) => void,
) {
  const pending = pendingProvider(input, currentProviders);
  // Server order is priority ascending, then creation time, so the new entry follows its peers.
  const insertAt = currentProviders.findIndex((provider) => provider.priority > pending.priority);
  const optimistic = [...currentProviders];
  optimistic.splice(insertAt === -1 ? optimistic.length : insertAt, 0, pending);
  mutate(SWR_KEYS.PROVIDERS, optimistic, false);

  try {
    const result = await api.createProvider(input);
    mutate(
      SWR_KEYS.PROVIDERS,
      (providers: Provider[] | undefined) =>
        (providers ?? optimistic).map((provider) => (provider.id === pending.id ? result : provider)),
      false,
    );
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.STATS);
    mutate(SWR_KEYS.CONFIG);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    return result;
  } catch (error) {
    mutate(
      SWR_KEYS.PROVIDERS,
      (providers: Provider[] | undefined) =>
        (providers ?? optimistic).filter((provider) => provider.id !== pending.id),
      false,
    );
    mutate(SWR_KEYS.PROVIDERS);
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
  onError?: (error: Error) => void,
) {
  const updatedProviders = currentProviders.map((p) =>
    p.id === id ? { ...p, ...input } : p,
  );
  mutate(SWR_KEYS.PROVIDERS, updatedProviders, false);

  try {
    const result = await api.updateProvider(id, input);
    mutate(providerDetailSWRKey(id), result, false);
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.CONFIG);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
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
  onError?: (error: Error) => void,
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
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(providerDetailSWRKey(id), undefined, false);
  } catch (error) {
    // Rollback on error
    mutate(SWR_KEYS.PROVIDERS, currentProviders, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// PL8: the list reflects the new order immediately so consecutive moves build
// on it; a failure revalidates from the server.
export async function reorderProviders(nextProviders: Provider[]) {
  mutate(
    SWR_KEYS.PROVIDERS,
    nextProviders.map((provider, priority) => ({ ...provider, priority })),
    false,
  );
  try {
    await api.reorderProviders(nextProviders.map((provider) => provider.id));
  } finally {
    mutate(SWR_KEYS.PROVIDERS);
  }
}

// CM-CH-EN-1: optimistic Channel enable toggle with rollback on failure.
export async function setChannelEnabledOptimistic(
  providerId: string,
  channelId: string,
  enabled: boolean,
) {
  const patch = (value: boolean) => (providers: Provider[] | undefined) =>
    providers?.map((provider) =>
      provider.id !== providerId
        ? provider
        : {
            ...provider,
            channels: provider.channels.map((channel) =>
              channel.id === channelId ? { ...channel, enabled: value } : channel,
            ),
          },
    );
  mutate(SWR_KEYS.PROVIDERS, patch(enabled), false);
  try {
    const result = await api.setChannelEnabled(providerId, channelId, enabled);
    mutate(providerDetailSWRKey(providerId), result, false);
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    return result;
  } catch (error) {
    mutate(SWR_KEYS.PROVIDERS, patch(!enabled), false);
    throw error;
  }
}

export async function upsertModelMetadataOptimistic(
  modelId: string,
  input: UpsertModelMetadataInput,
  currentRecords: ModelMetadataRecord[],
  onError?: (error: Error) => void,
) {
  const tempRecord: ModelMetadataRecord = {
    model_id: modelId,
    source: "manual",
    updated_at: new Date().toISOString(),
    raw_json: {},
    ...input,
    models_dev_provider: input.models_dev_provider ?? undefined,
    mode: input.mode ?? undefined,
    max_input_tokens: input.max_input_tokens ?? undefined,
    max_output_tokens: input.max_output_tokens ?? undefined,
    max_tokens: input.max_tokens ?? undefined,
  };
  const exists = currentRecords.some((r) => r.model_id === modelId);
  const optimistic = exists
    ? currentRecords.map((r) =>
        r.model_id === modelId ? { ...r, ...tempRecord } : r,
      )
    : [...currentRecords, tempRecord];
  mutate(SWR_KEYS.MODEL_METADATA, optimistic, false);

  try {
    const result = await api.upsertModelMetadata(modelId, input);
    mutate(SWR_KEYS.MODEL_METADATA);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
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
  onError?: (error: Error) => void,
) {
  const filtered = currentRecords.filter((r) => r.model_id !== modelId);
  mutate(SWR_KEYS.MODEL_METADATA, filtered, false);

  try {
    await api.deleteModelMetadata(modelId);
    mutate(SWR_KEYS.MODEL_METADATA);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
  } catch (error) {
    mutate(SWR_KEYS.MODEL_METADATA, currentRecords, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function syncModelMetadata(onError?: (error: Error) => void) {
  try {
    const result = await api.syncModelMetadataFromModelsDev();
    mutate(SWR_KEYS.MODEL_METADATA);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
    return result;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Model price mutation helpers (model-pricing.spec.md MP-UI2): optimistic
// list update, then revalidation of the price list and the unpriced set,
// which both depend on the mutated row.

export async function upsertModelPriceOptimistic(
  modelId: string,
  input: UpsertModelPriceInput,
  currentRecords: ModelPriceRecord[],
  onError?: (error: Error) => void,
) {
  const existing = currentRecords.find((r) => r.model_id === modelId);
  const tempRecord: ModelPriceRecord = {
    model_id: modelId,
    billing_mode: input.billing_mode ?? existing?.billing_mode ?? "per_token",
    input_usd_per_1m:
      input.input_usd_per_1m !== undefined
        ? input.input_usd_per_1m
        : (existing?.input_usd_per_1m ?? null),
    output_usd_per_1m:
      input.output_usd_per_1m !== undefined
        ? input.output_usd_per_1m
        : (existing?.output_usd_per_1m ?? null),
    cache_read_usd_per_1m:
      input.cache_read_usd_per_1m !== undefined
        ? input.cache_read_usd_per_1m
        : (existing?.cache_read_usd_per_1m ?? null),
    cache_write_usd_per_1m:
      input.cache_write_usd_per_1m !== undefined
        ? input.cache_write_usd_per_1m
        : (existing?.cache_write_usd_per_1m ?? null),
    cache_write_1h_usd_per_1m:
      input.cache_write_1h_usd_per_1m !== undefined
        ? input.cache_write_1h_usd_per_1m
        : (existing?.cache_write_1h_usd_per_1m ?? null),
    reasoning_usd_per_1m:
      input.reasoning_usd_per_1m !== undefined
        ? input.reasoning_usd_per_1m
        : (existing?.reasoning_usd_per_1m ?? null),
    per_request_usd:
      input.per_request_usd !== undefined
        ? input.per_request_usd
        : (existing?.per_request_usd ?? null),
    billing_expr:
      input.billing_expr !== undefined
        ? input.billing_expr
        : (existing?.billing_expr ?? null),
    source: existing?.source ?? "manual",
    locked_fields: input.locked_fields ?? existing?.locked_fields ?? [],
    raw_json: existing?.raw_json ?? {},
    enabled: input.enabled ?? existing?.enabled ?? true,
    updated_at: new Date().toISOString(),
  };
  const optimistic = existing
    ? currentRecords.map((r) => (r.model_id === modelId ? tempRecord : r))
    : [...currentRecords, tempRecord].sort((a, b) =>
        a.model_id.localeCompare(b.model_id),
      );
  mutate(SWR_KEYS.MODEL_PRICES, optimistic, false);

  try {
    const result = await api.upsertModelPrice(modelId, input);
    mutate(SWR_KEYS.MODEL_PRICES);
    mutate(SWR_KEYS.UNPRICED_MODELS);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
    return result;
  } catch (error) {
    mutate(SWR_KEYS.MODEL_PRICES, currentRecords, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteModelPriceOptimistic(
  modelId: string,
  currentRecords: ModelPriceRecord[],
  onError?: (error: Error) => void,
) {
  mutate(
    SWR_KEYS.MODEL_PRICES,
    currentRecords.filter((r) => r.model_id !== modelId),
    false,
  );

  try {
    await api.deleteModelPrice(modelId);
    mutate(SWR_KEYS.MODEL_PRICES);
    mutate(SWR_KEYS.UNPRICED_MODELS);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
  } catch (error) {
    mutate(SWR_KEYS.MODEL_PRICES, currentRecords, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function applyPriceSync(
  source: PriceSyncSource,
  previewId: string,
  onError?: (error: Error) => void,
) {
  try {
    const run = await api.applyPriceSync(source, previewId);
    mutate(SWR_KEYS.MODEL_PRICES);
    mutate(SWR_KEYS.UNPRICED_MODELS);
    mutate(SWR_KEYS.PRICE_SYNC_RUNS);
    mutate(SWR_KEYS.MODEL_METADATA);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
    mutate(SWR_KEYS.PROVIDERS);
    return run;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

/**
 * Pricing-tab settings write (model-pricing.spec.md MP-A8): sends only the
 * given keys, so unrelated settings are never overwritten with a stale copy.
 */
export async function patchPricingSettingsOptimistic(
  patch: Partial<SystemSettings>,
  onError?: (error: Error) => void,
) {
  mutate(
    SWR_KEYS.SETTINGS,
    (current: SystemSettings | undefined) => (current ? { ...current, ...patch } : current),
    false,
  );
  try {
    const updated = await api.updateSettings(patch);
    mutate(SWR_KEYS.SETTINGS, updated, false);
    return updated;
  } catch (error) {
    mutate(SWR_KEYS.SETTINGS);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Custom transform mutation helpers (custom-js-transforms.spec.md CJS-UI-2):
// every successful mutation also revalidates the transform registry so chain
// editors observe custom items without close/reopen.

export async function createCustomTransformOptimistic(
  source: string,
  onError?: (error: Error) => void,
) {
  try {
    const created = await api.createCustomTransform(source);
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS);
    mutate(SWR_KEYS.TRANSFORM_REGISTRY);
    mutate(SWR_KEYS.CUSTOM_TRANSFORM_REFERENCES);
    return created;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updateCustomTransformOptimistic(
  id: string,
  input: { source?: string; enabled?: boolean },
  currentTransforms: CustomTransform[],
  onError?: (error: Error) => void,
) {
  if (input.enabled !== undefined) {
    const optimistic = currentTransforms.map((item) =>
      item.id === id ? { ...item, enabled: input.enabled! } : item,
    );
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS, optimistic, false);
  }

  try {
    const updated = await api.updateCustomTransform(id, input);
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS);
    mutate(SWR_KEYS.TRANSFORM_REGISTRY);
    mutate(SWR_KEYS.CUSTOM_TRANSFORM_REFERENCES);
    return updated;
  } catch (error) {
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS, currentTransforms, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deleteCustomTransformOptimistic(
  id: string,
  currentTransforms: CustomTransform[],
  onError?: (error: Error) => void,
) {
  mutate(
    SWR_KEYS.CUSTOM_TRANSFORMS,
    currentTransforms.filter((item) => item.id !== id),
    false,
  );

  try {
    await api.deleteCustomTransform(id);
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS);
    mutate(SWR_KEYS.TRANSFORM_REGISTRY);
    mutate(SWR_KEYS.CUSTOM_TRANSFORM_REFERENCES);
  } catch (error) {
    mutate(SWR_KEYS.CUSTOM_TRANSFORMS, currentTransforms, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Recharge mutation helpers (recharge-system.spec.md §10-§11)

const isRechargeOrdersKey = (key: unknown): boolean =>
  typeof key === "string" && key.startsWith(SWR_KEYS.RECHARGE_ORDERS);

const isLedgerKey = (key: unknown): boolean =>
  typeof key === "string" && key.startsWith(SWR_KEYS.LEDGER);

/**
 * RC-W3: POST the order, then insert the returned pending order at the top of
 * the first-page orders cache before the caller navigates to `payment.url`.
 */
export async function createRechargeOrderOptimistic(
  input: CreateRechargeOrderInput,
  firstPageKey: string,
  onError?: (error: Error) => void,
): Promise<CreateRechargeOrderResponse> {
  try {
    const result = await api.createRechargeOrder(input);
    mutate(
      firstPageKey,
      (current: RechargeOrdersResponse | undefined) =>
        current
          ? {
              orders: [result.order, ...current.orders],
              total: current.total + 1,
            }
          : { orders: [result.order], total: 1 },
      false,
    );
    return result;
  } catch (error) {
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

/** Revalidate every order/ledger page plus the session user (RC-W6). */
export function revalidateRechargeCaches() {
  mutate(isRechargeOrdersKey);
  mutate(isLedgerKey);
  mutate(SWR_KEYS.ME);
  mutate(SWR_KEYS.STATS);
  mutate(SWR_KEYS.USERS);
}

export async function refundRechargeOrderOptimistic(
  order: RechargeOrder,
  manual: boolean,
  pageKey: string,
  onError?: (error: Error) => void,
): Promise<RechargeOrder> {
  let snapshot: RechargeOrdersResponse | undefined;
  await mutate(
    pageKey,
    (current: RechargeOrdersResponse | undefined) => {
      snapshot = current;
      if (!current) return current;
      return {
        ...current,
        orders: current.orders.map((row) =>
          row.id === order.id ? { ...row, status: "refunded" as const } : row,
        ),
      };
    },
    false,
  );

  try {
    const refunded = await api.refundRechargeOrder(order.id, manual);
    revalidateRechargeCaches();
    return refunded;
  } catch (error) {
    mutate(pageKey, snapshot, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function createPaymentChannelOptimistic(
  input: CreatePaymentChannelInput,
  currentChannels: PaymentChannel[],
  onError?: (error: Error) => void,
) {
  const now = new Date().toISOString();
  const tempChannel: PaymentChannel = {
    id: `temp-${Date.now()}`,
    name: input.name.trim(),
    type_id: input.type_id,
    enabled: input.enabled ?? true,
    currency: input.currency,
    usd_rate: input.usd_rate,
    min_credit_usd: input.min_credit_usd ?? "1",
    max_credit_usd: input.max_credit_usd ?? "10000",
    config: {},
    sort_order: input.sort_order ?? 0,
    created_at: now,
    updated_at: now,
  };
  mutate(SWR_KEYS.PAYMENT_CHANNELS, [...currentChannels, tempChannel], false);

  try {
    const created = await api.createPaymentChannel(input);
    mutate(SWR_KEYS.PAYMENT_CHANNELS);
    mutate(SWR_KEYS.RECHARGE_CHANNELS);
    return created;
  } catch (error) {
    mutate(SWR_KEYS.PAYMENT_CHANNELS, currentChannels, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function updatePaymentChannelOptimistic(
  id: string,
  input: UpdatePaymentChannelInput,
  currentChannels: PaymentChannel[],
  onError?: (error: Error) => void,
) {
  const optimistic = currentChannels.map((channel) =>
    channel.id === id
      ? {
          ...channel,
          name: input.name ?? channel.name,
          usd_rate: input.usd_rate ?? channel.usd_rate,
          min_credit_usd: input.min_credit_usd ?? channel.min_credit_usd,
          max_credit_usd: input.max_credit_usd ?? channel.max_credit_usd,
          enabled: input.enabled ?? channel.enabled,
          sort_order: input.sort_order ?? channel.sort_order,
          updated_at: new Date().toISOString(),
        }
      : channel,
  );
  mutate(SWR_KEYS.PAYMENT_CHANNELS, optimistic, false);

  try {
    const updated = await api.updatePaymentChannel(id, input);
    mutate(SWR_KEYS.PAYMENT_CHANNELS);
    mutate(SWR_KEYS.RECHARGE_CHANNELS);
    return updated;
  } catch (error) {
    mutate(SWR_KEYS.PAYMENT_CHANNELS, currentChannels, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

export async function deletePaymentChannelOptimistic(
  id: string,
  currentChannels: PaymentChannel[],
  onError?: (error: Error) => void,
) {
  mutate(
    SWR_KEYS.PAYMENT_CHANNELS,
    currentChannels.filter((channel) => channel.id !== id),
    false,
  );

  try {
    await api.deletePaymentChannel(id);
    mutate(SWR_KEYS.PAYMENT_CHANNELS);
    mutate(SWR_KEYS.RECHARGE_CHANNELS);
  } catch (error) {
    mutate(SWR_KEYS.PAYMENT_CHANNELS, currentChannels, false);
    if (onError && error instanceof Error) {
      onError(error);
    }
    throw error;
  }
}

// Global revalidation helpers
export function revalidateAll() {
  return mutate(() => true);
}

export function clearCache() {
  return mutate(() => true, undefined, { revalidate: false });
}

// `keepCurrentUser` leaves SWR_KEYS.ME in place when the session rotates, so the
// shell never observes a logged-out gap and redirects to /login.
export function clearAuthenticatedCache(options?: { keepCurrentUser?: boolean }) {
  return mutate(
    (key) =>
      key !== SWR_KEYS.PUBLIC_SETTINGS &&
      !(options?.keepCurrentUser && key === SWR_KEYS.ME),
    undefined,
    { revalidate: false },
  );
}
