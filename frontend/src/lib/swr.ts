import useSWR, { mutate } from "swr";
import type { SWRConfiguration } from "swr";
import { api } from "./api";
import { usageWindowStartIso, type UsageWindow } from "./usage-window";
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
  BillingPlan,
  BillingPlanInput,
  BillingPlanSubscription,
  Group,
  CreateGroupInput,
  UpdateGroupInput,
  UserLiveUsage,
  CustomTransform,
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
  unpricedModels: async () => (await api.listUnpricedModels()).models,
  priceSyncRuns: () => api.listPriceSyncRuns(),
  billingPlans: () => api.listBillingPlans(),
  billingPlanMarketplace: () => api.listBillingPlanMarketplace(),
  billingPlanSubscription: () => api.getBillingPlanSubscription(),
  marketplaceModels: () => api.listMarketplaceModels(),
  customTransforms: () => api.listCustomTransforms(),
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

export function useModelMetadata(config?: SWRConfiguration) {
  return useSWR<MarketplaceModelRecord[]>(
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
  return useSWR<string[]>(SWR_KEYS.UNPRICED_MODELS, fetchers.unpricedModels, {
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

// Recent-usage request logs scoped to a time window (dashboard-home-overview
// spec DH-7a). The SWR key carries only the window token so the cache entry is
// stable per selection; time bounds are evaluated at fetch time so every
// revalidation observes logs created after the window was selected.
// keepPreviousData satisfies DH-12b (no skeleton flash on window switch).
export function useWindowedRequestLogs(
  window: UsageWindow,
  limit = 200,
  config?: SWRConfiguration,
) {
  return useSWR<RequestLogsResponse>(
    `${SWR_KEYS.REQUEST_LOGS}?window=${window}&limit=${limit}`,
    () => {
      const now = new Date();
      return api.listRequestLogs(limit, 0, {
        time_from: usageWindowStartIso(window, now),
        time_to: now.toISOString(),
      });
    },
    { ...defaultConfig, keepPreviousData: true, ...config },
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
  return useSWR<DashboardAnalytics>(
    `${SWR_KEYS.ANALYTICS}?buckets=${buckets}&range_hours=${rangeHours}`,
    () => api.getDashboardAnalytics(buckets, rangeHours),
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
export function useAdminOverview(config?: SWRConfiguration) {
  return useSWR<AdminOverview>(
    SWR_KEYS.ADMIN_OVERVIEW,
    () => api.getAdminOverview(),
    {
      ...defaultConfig,
      refreshInterval: 10000,
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

export async function updateSettingsOptimistic(
  newSettings: SystemSettings,
  onError?: (error: Error) => void,
) {
  // Optimistic update
  mutate(SWR_KEYS.SETTINGS, newSettings, false);

  try {
    const updated = await api.updateSettings(newSettings);
    // Revalidate with server data
    mutate(SWR_KEYS.SETTINGS, updated, false);
    mutate(SWR_KEYS.PUBLIC_SETTINGS);
    mutate(SWR_KEYS.PROVIDERS);
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
  updates: Partial<User> & { password?: string },
  currentUsers: User[],
  onError?: (error: Error) => void,
) {
  // Optimistic update
  const updatedUsers = currentUsers.map((u) =>
    u.id === userId ? { ...u, ...updates } : u,
  );
  mutate(SWR_KEYS.USERS, updatedUsers, false);

  try {
    await api.updateUser(userId, updates);
    // Revalidate to get the real data
    mutate(SWR_KEYS.USERS);
    mutate(SWR_KEYS.ME);
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
    sort_order: input.sort_order ?? 0,
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
export async function createProviderOptimistic(
  input: CreateProviderInput,
  _currentProviders: Provider[],
  onError?: (error: Error) => void,
) {
  try {
    const result = await api.createProvider(input);
    // Revalidate to get the new provider
    mutate(SWR_KEYS.PROVIDERS);
    mutate(SWR_KEYS.STATS);
    mutate(SWR_KEYS.CONFIG);
    mutate(SWR_KEYS.MARKETPLACE_MODELS);
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

export async function reorderProviders(
  providerIds: string[],
  onError?: (error: Error) => void,
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
  onError?: (error: Error) => void,
) {
  try {
    const run = await api.applyPriceSync(source);
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
