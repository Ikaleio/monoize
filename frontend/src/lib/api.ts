const API_BASE = "/api/dashboard";

type UnauthorizedHandler = () => void;

const unauthorizedHandlers = new Set<UnauthorizedHandler>();

export function subscribeDashboardUnauthorized(handler: UnauthorizedHandler): () => void {
  unauthorizedHandlers.add(handler);
  return () => {
    unauthorizedHandlers.delete(handler);
  };
}

function notifyDashboardUnauthorized(): void {
  for (const handler of unauthorizedHandlers) {
    handler();
  }
}

export interface Group {
  id: string;
  name: string;
  description: string;
  is_default: boolean;
  user_selectable: boolean;
  sort_order: number;
  created_at: string;
  updated_at: string;
}

export interface CreateGroupInput {
  name: string;
  description?: string;
  user_selectable?: boolean;
  sort_order?: number;
}

export interface UpdateGroupInput {
  name?: string;
  description?: string;
  user_selectable?: boolean;
  sort_order?: number;
}

export interface UserBillingPlan {
  id: string;
  name: string;
  grant_amount_nano_usd: string;
  grant_amount_usd: string;
  schedule: string;
  group_ids: string[];
  enabled: boolean;
}

export interface User {
  id: string;
  username: string;
  role: "super_admin" | "admin" | "user";
  created_at: string;
  last_login_at?: string;
  enabled: boolean;
  balance_nano_usd: string;
  balance_usd: string;
  balance_unlimited: boolean;
  email?: string | null;
  group_id: string;
  billing_plan_id?: string | null;
  next_grant_at?: string | null;
  billing_plan?: UserBillingPlan | null;
  today_calls?: number;
  today_cost_nano_usd?: string;
  today_cost_usd?: string;
}

export interface BillingPlan {
  id: string;
  name: string;
  grant_amount_nano_usd: string;
  grant_amount_usd: string;
  schedule: string;
  group_ids: string[];
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface BillingPlanInput {
  name: string;
  grant_amount_nano_usd?: string;
  grant_amount_usd?: string;
  schedule: string;
  group_ids?: string[];
  enabled?: boolean;
}

export interface AuthResponse {
  user: User;
}

export type Phase = "request" | "response";

export type TransformScope = "provider" | "global" | "api_key";

export interface TransformRuleConfig {
  transform: string;
  enabled: boolean;
  models?: string[] | null;
  phase: Phase;
  config: Record<string, unknown>;
}

export interface TransformRegistryItem {
  type_id: string;
  supported_phases: Phase[];
  supported_scopes: TransformScope[];
  config_schema: Record<string, unknown>;
  name: Record<string, string>;
  description: Record<string, string>;
}

export interface ModelRedirectRule {
  pattern: string;
  replace: string;
}

export type RequestCaptureMode = "off" | "capture-all" | "capture-only-abnormal";

export interface ApiKey {
  id: string;
  name: string;
  key_prefix: string;
  key: string;
  created_at: string;
  expires_at?: string;
  last_used_at?: string;
  enabled: boolean;
  sub_account_enabled: boolean;
  sub_account_balance_nano_usd: string;
  sub_account_balance_usd: string;
  model_limits_enabled: boolean;
  model_limits: string[];
  ip_whitelist: string[];
  use_user_group: boolean;
  group_ids: string[];
  max_multiplier?: string;
  transforms: TransformRuleConfig[];
  model_redirects: ModelRedirectRule[];
  reasoning_envelope_enabled: boolean;
  request_capture_mode: RequestCaptureMode;
}

export type ApiKeyCreated = ApiKey;

export interface CreateApiKeyInput {
  name: string;
  expires_in_days?: number;
  sub_account_enabled?: boolean;
  sub_account_balance_nano_usd?: string;
  model_limits_enabled?: boolean;
  model_limits?: string[];
  ip_whitelist?: string[];
  use_user_group?: boolean;
  group_ids?: string[];
  max_multiplier?: string;
  transforms?: TransformRuleConfig[];
  model_redirects?: ModelRedirectRule[];
  reasoning_envelope_enabled?: boolean;
  request_capture_mode?: RequestCaptureMode;
}

export interface UpdateApiKeyInput {
  name?: string;
  enabled?: boolean;
  sub_account_enabled?: boolean;
  sub_account_balance_nano_usd?: string;
  model_limits_enabled?: boolean;
  model_limits?: string[];
  ip_whitelist?: string[];
  use_user_group?: boolean;
  group_ids?: string[];
  max_multiplier?: string;
  transforms?: TransformRuleConfig[];
  expires_at?: string;
  model_redirects?: ModelRedirectRule[];
  reasoning_envelope_enabled?: boolean;
  request_capture_mode?: RequestCaptureMode;
}

export interface SystemSettings {
  registration_enabled: boolean;
  captcha_enabled: boolean;
  default_user_role: string;
  session_ttl_days: number;
  api_key_max_per_user: number;
  site_name: string;
  site_description: string;
  api_base_url: string;
  models_dev_base_url: string;
  global_transforms: TransformRuleConfig[];
  global_model_redirects: ModelRedirectRule[];
  reasoning_suffix_map: Record<string, string>;
  codex_model_ids: string[];
  monoize_active_probe_enabled: boolean;
  monoize_active_probe_interval_seconds: number;
  monoize_active_probe_success_threshold: number;
  monoize_active_probe_model?: string | null;
  monoize_affinity_enabled: boolean;
  monoize_affinity_idle_ttl_seconds: number;
  monoize_affinity_failback_mode: AffinityFailbackMode;
  monoize_affinity_failback_delay_seconds: number;
  monoize_passive_failure_threshold: number;
  monoize_passive_cooldown_seconds: number;
  monoize_passive_window_seconds: number;
  monoize_passive_min_samples: number;
  monoize_passive_failure_rate_threshold: number;
  monoize_passive_rate_limit_cooldown_seconds: number;
  monoize_request_timeout_ms: number;
  monoize_enable_estimated_billing: boolean;
  monoize_extra_fields_whitelist: Record<string, string[]>;
  monoize_strip_cross_protocol_nested_extra: boolean;
  monoize_request_capture_enabled: boolean;
  monoize_request_capture_retention_days: number;
  monoize_mask_sensitive_info: boolean;
  pricing_profile_model_patterns: PricingProfilePattern[];
  updated_at: string;
}

export interface PublicSystemSettings {
  registration_enabled: boolean;
  captcha_enabled: boolean;
  site_name: string;
  site_description: string;
  api_base_url: string;
  cap_api_endpoint: string | null;
}

export interface DashboardStats {
  user_count: number | null;
  my_api_keys_count: number;
  providers_count: number;
  config_providers_count: number;
  current_user: User;
}

export interface DashboardGroupsResponse {
  groups: Group[];
}

export interface ConfigOverview {
  server: {
    listen: string;
    metrics_path: string;
    unknown_fields_policy: string;
  };
  database: {
    dsn: string;
  };
  routing?: {
    providers_count: number;
  };
  providers?: Array<{
    id: string;
    type: string;
    has_base_url: boolean;
    model_count: number;
    member_count: number;
  }>;
  model_registry?: {
    sources_count: number;
  };
}

export interface MonoizeModelEntry {
  redirect: string | null;
  multiplier: string;
}

export interface MonoizeChannel {
  id: string;
  name: string;
  provider_type: ProviderType;
  base_url: string;
  weight: number;
  enabled: boolean;
  allow_missing_usage: boolean;
  passive_failure_count_threshold_override?: number | null;
  passive_cooldown_seconds_override?: number | null;
  passive_window_seconds_override?: number | null;
  passive_rate_limit_cooldown_seconds_override?: number | null;
  models: Record<string, MonoizeModelEntry>;
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  affinity_enabled_override?: boolean | null;
  affinity_idle_ttl_seconds_override?: number | null;
  affinity_failback_mode_override?: AffinityFailbackMode | null;
  affinity_failback_delay_seconds_override?: number | null;
  proxy_url?: string | null;
  extra_headers?: Record<string, string> | null;
  session_affinity_auto?: boolean | null;
  _healthy?: boolean;
  _last_success_at?: string;
  _health_status?: "healthy" | "probing" | "unhealthy";
  _unhealthy_models?: string[];
  _probing_models?: string[];
  _cooldown_until?: string;
}

export interface ProviderModelRuntimeChannel {
  channel_id: string;
  channel_name: string;
  cooldown_until?: string;
}

export interface ProviderModelRuntimeStatus {
  model: string;
  availability_status: "healthy" | "degraded" | "unavailable";
  eligible_channel_count: number;
  available_channel_count: number;
  breaker_channels: ProviderModelRuntimeChannel[];
  pricing_status: "complete" | "partial" | "missing";
  unpriced_channels: ProviderModelRuntimeChannel[];
}

export type ProviderType = "responses" | "chat_completion" | "messages" | "gemini" | "openai_image" | "replicate";
export type AffinityFailbackMode = "sticky" | "prefer_higher_priority";

export interface ApiTypeOverride {
  pattern: string;
  api_type: ProviderType;
}
export interface Provider {
  id: string;
  name: string;
  channels: MonoizeChannel[];
  max_retries: number;
  channel_max_retries: number;
  channel_retry_interval_ms: number;
  circuit_breaker_enabled: boolean;
  per_model_circuit_break: boolean;
  transforms: TransformRuleConfig[];
  api_type_overrides: ApiTypeOverride[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  request_timeout_ms_override?: number | null;
  extra_fields_whitelist?: string[] | null;
  strip_cross_protocol_nested_extra?: boolean | null;
  group_ids: string[];
  enabled: boolean;
  priority: number;
  created_at: string;
  updated_at: string;
  unpriced_model_count?: number;
  unpriced_model_ids?: string[];
  model_runtime_statuses?: ProviderModelRuntimeStatus[];
}

export interface CreateMonoizeChannelInput {
  id?: string;
  name: string;
  provider_type: ProviderType;
  base_url: string;
  api_key?: string;
  weight?: number;
  enabled?: boolean;
  allow_missing_usage?: boolean;
  passive_failure_count_threshold_override?: number | null;
  passive_cooldown_seconds_override?: number | null;
  passive_window_seconds_override?: number | null;
  passive_rate_limit_cooldown_seconds_override?: number | null;
  models: Record<string, MonoizeModelEntry>;
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  affinity_enabled_override?: boolean | null;
  affinity_idle_ttl_seconds_override?: number | null;
  affinity_failback_mode_override?: AffinityFailbackMode | null;
  affinity_failback_delay_seconds_override?: number | null;
  proxy_url?: string | null;
  extra_headers?: Record<string, string> | null;
  session_affinity_auto?: boolean | null;
}

export interface CreateProviderInput {
  name: string;
  channels: CreateMonoizeChannelInput[];
  max_retries?: number;
  channel_max_retries?: number;
  channel_retry_interval_ms?: number;
  circuit_breaker_enabled?: boolean;
  per_model_circuit_break?: boolean;
  transforms?: TransformRuleConfig[];
  api_type_overrides?: ApiTypeOverride[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  request_timeout_ms_override?: number | null;
  extra_fields_whitelist?: string[] | null;
  strip_cross_protocol_nested_extra?: boolean | null;
  group_ids?: string[];
  enabled?: boolean;
  priority?: number;
}

export interface UpdateProviderInput {
  name?: string;
  channels?: CreateMonoizeChannelInput[];
  max_retries?: number;
  channel_max_retries?: number;
  channel_retry_interval_ms?: number;
  circuit_breaker_enabled?: boolean;
  per_model_circuit_break?: boolean;
  transforms?: TransformRuleConfig[];
  api_type_overrides?: ApiTypeOverride[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  request_timeout_ms_override?: number | null;
  extra_fields_whitelist?: string[] | null;
  strip_cross_protocol_nested_extra?: boolean | null;
  group_ids?: string[];
  enabled?: boolean;
  priority?: number;
}

export interface ModelMetadataRecord {
  model_id: string;
  models_dev_provider?: string;
  mode?: string;
  input_cost_per_token_nano?: string;
  output_cost_per_token_nano?: string;
  cache_read_input_cost_per_token_nano?: string;
  cache_creation_input_cost_per_token_nano?: string;
  output_cost_per_reasoning_token_nano?: string;
  max_input_tokens?: number;
  max_output_tokens?: number;
  max_tokens?: number;
  raw_json: Record<string, unknown>;
  source: string;
  updated_at: string;
}

export interface UpsertModelMetadataInput {
  models_dev_provider?: string | null;
  mode?: string | null;
  input_cost_per_token_nano?: string | null;
  output_cost_per_token_nano?: string | null;
  cache_read_input_cost_per_token_nano?: string | null;
  cache_creation_input_cost_per_token_nano?: string | null;
  output_cost_per_reasoning_token_nano?: string | null;
  max_input_tokens?: number | null;
  max_output_tokens?: number | null;
  max_tokens?: number | null;
}

export interface ModelMetadataSyncResult {
  success: boolean;
  upserted: number;
  skipped: number;
  deleted: number;
  fetched_at: string;
}

export interface BatchDeleteModelMetadataResult {
  success: boolean;
  deleted: number;
  not_found: string[];
}

export interface BillingRateRecord {
  id: string;
  source: string;
  pricing_profile: string;
  model_pattern?: string | null;
  provider_type?: string | null;
  rate_kind: string;
  usage_class: string;
  unit: string;
  unit_price_nano_usd: string;
  context_tier?: string | null;
  service_tier?: string | null;
  modality?: string | null;
  cache_ttl?: string | null;
  match_json: Record<string, unknown>;
  priority: number;
  enabled: boolean;
  raw_json: Record<string, unknown>;
  updated_at: string;
}

export interface UpsertBillingRateInput {
  source?: string;
  pricing_profile?: string;
  model_pattern?: string | null;
  provider_type?: string | null;
  rate_kind?: string;
  usage_class?: string;
  unit?: string;
  unit_price_nano_usd?: string;
  context_tier?: string | null;
  service_tier?: string | null;
  modality?: string | null;
  cache_ttl?: string | null;
  match_json?: Record<string, unknown>;
  priority?: number;
  enabled?: boolean;
  raw_json?: Record<string, unknown>;
}

export interface BillingRateSyncResult {
  success: boolean;
  upserted: number;
  skipped: number;
  deleted: number;
  fetched_at: string;
}

export interface PricingProfilePattern {
  pattern: string;
  pricing_profile: string;
}

export interface PricingProfilePatternsResponse {
  patterns: PricingProfilePattern[];
}

export interface RequestLogProvider {
  id?: string;
  name?: string;
  multiplier?: string;
}

export interface RequestLogChannel {
  id?: string;
  name?: string;
}

export interface RequestLogUser {
  id: string;
  username?: string;
}

export interface RequestLogApiKey {
  id?: string;
  name?: string;
}

export interface RequestLogTokens {
  input?: number;
  output?: number;
  cache_read?: number;
  cache_creation?: number;
  tool_prompt?: number;
  reasoning?: number;
  accepted_prediction?: number;
  rejected_prediction?: number;
}

export interface RequestLogTiming {
  duration_ms?: number | string | null;
  ttfb_ms?: number | string | null;
}

export interface RequestLogBilling {
  charge_nano_usd?: string;
  breakdown?: Record<string, unknown>;
}

export interface RequestLogError {
  code?: string;
  message?: string;
  http_status?: number;
}

export interface RequestLogAffinity {
  hit?: boolean;
  key_hash?: string;
  target?: string;
}

export interface RequestLogTriedProvider {
  attempt_number?: number;
  provider_id: string;
  channel_id: string;
  provider_name?: string | null;
  channel_name?: string | null;
  error: string;
  upstream_status?: number | null;
  upstream_code?: string | null;
  upstream_type?: string | null;
  upstream_param?: string | null;
  duration_ms?: number | null;
}

export interface RequestLog {
  id: string;
  request_id?: string;
  created_at: string;
  status: string;
  is_stream: boolean;
  model: string;
  upstream_model?: string;
  effective_provider_type?: string;
  request_kind?: string;
  reasoning_effort?: string;
  request_ip?: string;
  tried_providers?: RequestLogTriedProvider[];
  session_affinity_value?: string | null;
  provider: RequestLogProvider;
  channel: RequestLogChannel;
  affinity?: RequestLogAffinity;
  user: RequestLogUser;
  api_key: RequestLogApiKey;
  tokens: RequestLogTokens;
  timing: RequestLogTiming;
  billing: RequestLogBilling;
  usage?: Record<string, unknown>;
  error: RequestLogError;
  has_capture?: boolean;
}

export interface CaptureTransformChainEntry {
  scope: "provider" | "global" | "api_key";
  transform: string;
  phase: "request" | "response";
}

export interface CaptureFrameTruncation {
  truncated?: boolean;
  omitted_frames?: number;
  omitted_bytes?: number;
  retained_frames?: number;
  retained_bytes?: number;
}

export interface CaptureAttempt {
  attempt_number: number;
  provider_id: string;
  channel_id?: string | null;
  provider_type: string;
  logical_model: string;
  upstream_model: string;
  upstream_path: string;
  raw_input: unknown;
  transformed_urp_request: unknown;
  upstream_request: unknown;
  downstream_response?: unknown;
  downstream_sse_frames?: string[] | null;
  downstream_sse_frames_truncation?: CaptureFrameTruncation;
  transform_chain?: CaptureTransformChainEntry[] | null;
  /** Number of transform entries redacted for non-admin viewers (RCV-A10). */
  hidden_transforms?: number;
  error?: unknown;
}

export interface CaptureDump {
  version: number;
  request_id?: string | null;
  created_at: string;
  api_key_id: string;
  user_id: string;
  downstream_protocol: string;
  is_stream: boolean;
  attempts: CaptureAttempt[];
  capture_truncation?: Record<string, unknown>;
}

export interface RequestCaptureDetail {
  request_id: string;
  file_name: string;
  created_at: string;
  size_bytes: number;
  owner: { id: string; username?: string | null };
  dump: CaptureDump;
}

export interface AdminOverviewNode {
  role: string;
  version: string;
  started_at: string;
  uptime_seconds: number;
  listen: string;
  metrics_path: string;
  database_backend: string;
  database_dsn_redacted: string;
  upstream_proxy_url?: string | null;
}

export interface AdminOverviewReplicaNode {
  id: string;
  hostname: string;
  listen: string;
  version: string;
  started_at: string;
  last_seen_at: string;
  uptime_seconds: number;
  spool_pending_count: number;
  spool_pending_bytes: number;
  stale: boolean;
}

export interface AdminOverviewReplica {
  ingest_enabled: boolean;
  spool_pending_count: number;
  spool_pending_bytes: number;
  replicas: AdminOverviewReplicaNode[];
}

export interface AdminOverviewToday {
  calls: number;
  cost_nano_usd: string;
}

export interface AdminOverviewSystem {
  pending_request_logs: number;
  sse_connections: number;
  channel_health_entries: number;
  channel_affinity_entries: number;
  routing_config_revision: string;
}

export interface AdminOverviewUserRanking {
  user_id: string;
  username?: string | null;
  call_count: number;
  cost_nano_usd: string;
}

export interface AdminOverviewChannelHealth {
  provider_id: string;
  provider_name: string;
  channel_id: string;
  channel_name: string;
  enabled: boolean;
  weight: number;
  session_affinity_auto: boolean;
  healthy: boolean;
  last_success_at?: number | null;
  cooldown_until?: number | null;
  probe_success_count: number;
  last_probe_at?: number | null;
  cooldown_active: boolean;
  unhealthy_models: string[];
  today_calls: number;
  today_cost_nano_usd: string;
}

export interface AdminOverview {
  node: AdminOverviewNode;
  replica: AdminOverviewReplica;
  system: AdminOverviewSystem;
  today: AdminOverviewToday;
  users_ranking: AdminOverviewUserRanking[];
  channel_health: AdminOverviewChannelHealth[];
}

export interface DashboardAnalyticsBucket {
  label: string;
  cost_by_model: Record<string, string>;
  calls_by_model: Record<string, number>;
  calls_by_provider: Record<string, number>;
}

export interface DashboardAnalytics {
  buckets: DashboardAnalyticsBucket[];
  time_from: string;
  time_to: string;
  total_cost_nano_usd: string;
  total_calls: number;
  today_cost_nano_usd: string;
  today_calls: number;
}

// Rolling 60-second own-usage aggregate (user-live-usage.spec.md LU-6).
export interface UserLiveUsage {
  window_seconds: number;
  rpm: number;
  tpm: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_hit_rate: number | null;
}

export interface RequestLogsFilter {
  model?: string;
  status?: string;
  api_key_id?: string;
  username?: string;
  search?: string;
  time_from?: string;
  time_to?: string;
}

export interface RequestLogsResponse {
  data: RequestLog[];
  total: number;
  limit: number;
  offset: number;
  total_charge_nano_usd: string;
}

export interface ChannelTestResult {
  success: boolean;
  latency_ms: number;
  model: string;
  stream: boolean;
  http_status: number | null;
  error_code: string | null;
  error_type: string | null;
  error: string | null;
}

export interface FetchChannelModelsInput {
  provider_type: ProviderType;
  base_url: string;
  api_key?: string;
  provider_id?: string;
  channel_id?: string;
}

class ApiClient {
  private async request<T>(
    path: string,
    options: RequestInit = {}
  ): Promise<T> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...(options.headers as Record<string, string>),
    };

    const response = await fetch(`${API_BASE}${path}`, {
      ...options,
      headers,
      credentials: "include",
    });

    const data = await response.json();

    if (!response.ok) {
      if (response.status === 401 && data.error?.code === "unauthorized") {
        notifyDashboardUnauthorized();
      }
      throw new Error(data.error?.message || data.error?.code || "Request failed");
    }

    return data;
  }

  // Auth
  async register(username: string, password: string, captchaToken: string): Promise<AuthResponse> {
    return this.request("/auth/register", {
      method: "POST",
      body: JSON.stringify({ username, password, captcha_token: captchaToken }),
    });
  }

  async login(username: string, password: string, captchaToken: string): Promise<AuthResponse> {
    return this.request("/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password, captcha_token: captchaToken }),
    });
  }

  async logout(): Promise<void> {
    await this.request("/auth/logout", { method: "POST" });
  }

  async me(): Promise<User> {
    return this.request("/auth/me");
  }

  async changePassword(currentPassword: string, newPassword: string): Promise<AuthResponse> {
    return this.request("/auth/password", {
      method: "PUT",
      body: JSON.stringify({
        current_password: currentPassword,
        new_password: newPassword,
      }),
    });
  }

  // Users
  async listUsers(): Promise<User[]> {
    return this.request("/users");
  }

  async getUser(id: string): Promise<User> {
    return this.request(`/users/${id}`);
  }

  async createUser(
    username: string,
    password: string,
    role?: string,
    group_id?: string
  ): Promise<User> {
    return this.request("/users", {
      method: "POST",
      body: JSON.stringify({ username, password, role, group_id }),
    });
  }

  async updateUser(
    id: string,
    updates: {
      username?: string;
      password?: string;
      role?: string;
      enabled?: boolean;
      balance_nano_usd?: string;
      balance_usd?: string;
      balance_unlimited?: boolean;
      email?: string | null;
      group_id?: string;
      billing_plan_id?: string | null;
    }
  ): Promise<User> {
    return this.request(`/users/${id}`, {
      method: "PUT",
      body: JSON.stringify(updates),
    });
  }

  // Billing plans
  async listBillingPlans(): Promise<BillingPlan[]> {
    return this.request("/billing-plans");
  }

  async createBillingPlan(input: BillingPlanInput): Promise<BillingPlan> {
    return this.request("/billing-plans", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  async updateBillingPlan(id: string, input: BillingPlanInput): Promise<{ success: boolean }> {
    return this.request(`/billing-plans/${id}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async deleteBillingPlan(id: string): Promise<{ success: boolean }> {
    await this.request(`/billing-plans/${id}`, { method: "DELETE" });
    return { success: true };
  }

  async resetBillingPlan(
    id: string
  ): Promise<{ success: boolean; reset_count: number }> {
    return this.request(`/billing-plans/${id}/reset`, { method: "POST" });
  }

  async updateMe(updates: { email?: string | null }): Promise<User> {
    return this.request("/auth/me", {
      method: "PUT",
      body: JSON.stringify(updates),
    });
  }

  async deleteUser(id: string): Promise<void> {
    await this.request(`/users/${id}`, { method: "DELETE" });
  }

  // API Keys
  async listApiKeys(): Promise<ApiKey[]> {
    return this.request("/tokens");
  }

  async getApiKey(id: string): Promise<ApiKey> {
    return this.request(`/tokens/${id}`);
  }

  async createApiKey(input: CreateApiKeyInput): Promise<ApiKeyCreated> {
    return this.request("/tokens", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  async updateApiKey(id: string, input: UpdateApiKeyInput): Promise<ApiKey> {
    return this.request(`/tokens/${id}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async deleteApiKey(id: string): Promise<void> {
    await this.request(`/tokens/${id}`, { method: "DELETE" });
  }

  async batchDeleteApiKeys(ids: string[]): Promise<{ success: boolean; deleted_count: number }> {
    return this.request("/tokens/batch-delete", {
      method: "POST",
      body: JSON.stringify({ ids }),
    });
  }

  async transferToSubAccount(keyId: string, input: { amount_nano_usd?: string; amount_usd?: string }): Promise<{ success: boolean; api_key_balance_nano_usd: string; user_balance_nano_usd: string }> {
    return this.request(`/tokens/${keyId}/transfer`, {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  // Settings
  async getSettings(): Promise<SystemSettings> {
    return this.request("/settings");
  }

  async updateSettings(
    settings: Partial<SystemSettings>
  ): Promise<SystemSettings> {
    return this.request("/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    });
  }

  async getPublicSettings(): Promise<PublicSystemSettings> {
    return this.request("/settings/public");
  }

  // Dashboard
  async getStats(): Promise<DashboardStats> {
    return this.request("/stats");
  }

  async getConfigOverview(): Promise<ConfigOverview> {
    return this.request("/config");
  }

  async listDashboardGroups(): Promise<DashboardGroupsResponse> {
    return this.request("/groups");
  }

  // Groups registry (admin CRUD)
  async createGroup(input: CreateGroupInput): Promise<Group> {
    return this.request("/groups", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  async updateGroup(id: string, input: UpdateGroupInput): Promise<Group> {
    return this.request(`/groups/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async reorderGroups(groupIds: string[]): Promise<{ success: boolean }> {
    return this.request("/groups/reorder", {
      method: "POST",
      body: JSON.stringify({ group_ids: groupIds }),
    });
  }

  async deleteGroup(id: string): Promise<{ success: boolean }> {
    return this.request(`/groups/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  }

  // Providers
  async listProviders(): Promise<Provider[]> {
    return this.request("/providers");
  }

  async getProvider(id: string): Promise<Provider> {
    return this.request(`/providers/${id}`);
  }

  async createProvider(input: CreateProviderInput): Promise<Provider> {
    return this.request("/providers", {
      method: "POST",
      body: JSON.stringify(input),
    });
  }

  async updateProvider(id: string, input: UpdateProviderInput): Promise<Provider> {
    return this.request(`/providers/${id}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async deleteProvider(id: string): Promise<void> {
    await this.request(`/providers/${id}`, { method: "DELETE" });
  }

  async reorderProviders(providerIds: string[]): Promise<void> {
    await this.request("/providers/reorder", {
      method: "POST",
      body: JSON.stringify({ provider_ids: providerIds }),
    });
  }

  async getTransformRegistry(): Promise<TransformRegistryItem[]> {
    return this.request("/transforms/registry");
  }

  async listModelMetadata(): Promise<ModelMetadataRecord[]> {
    return this.request("/model-metadata");
  }

  async getModelMetadata(modelId: string): Promise<ModelMetadataRecord> {
    return this.request(`/model-metadata/${encodeURIComponent(modelId)}`);
  }

  async upsertModelMetadata(
    modelId: string,
    input: UpsertModelMetadataInput
  ): Promise<ModelMetadataRecord> {
    return this.request(`/model-metadata/${encodeURIComponent(modelId)}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async deleteModelMetadata(modelId: string): Promise<{ success: boolean }> {
    return this.request(`/model-metadata/${encodeURIComponent(modelId)}`, {
      method: "DELETE",
    });
  }

  async batchDeleteModelMetadata(
    modelIds: string[]
  ): Promise<BatchDeleteModelMetadataResult> {
    return this.request("/model-metadata/batch-delete", {
      method: "POST",
      body: JSON.stringify({ model_ids: modelIds }),
    });
  }

  async syncModelMetadataFromModelsDev(): Promise<ModelMetadataSyncResult> {
    return this.request("/model-metadata/sync/models-dev", {
      method: "POST",
    });
  }

  async listBillingRates(): Promise<BillingRateRecord[]> {
    return this.request("/billing-rates");
  }

  async upsertBillingRate(
    id: string,
    input: UpsertBillingRateInput
  ): Promise<BillingRateRecord> {
    return this.request(`/billing-rates/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: JSON.stringify(input),
    });
  }

  async deleteBillingRate(id: string): Promise<{ success: boolean }> {
    return this.request(`/billing-rates/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  }

  async syncBillingRatesCatalog(): Promise<BillingRateSyncResult> {
    return this.request("/billing-rates/sync/catalog", {
      method: "POST",
    });
  }

  async getPricingProfilePatterns(): Promise<PricingProfilePatternsResponse> {
    return this.request("/pricing-profile-patterns");
  }

  async updatePricingProfilePatterns(
    patterns: PricingProfilePattern[]
  ): Promise<PricingProfilePatternsResponse> {
    return this.request("/pricing-profile-patterns", {
      method: "PUT",
      body: JSON.stringify({ patterns }),
    });
  }

  async fetchChannelModels(input: FetchChannelModelsInput): Promise<{
    models: string[];
  }> {
    const body: FetchChannelModelsInput = {
      provider_type: input.provider_type,
      base_url: input.base_url,
    };
    if (input.api_key?.trim()) body.api_key = input.api_key.trim();
    if (input.provider_id?.trim()) body.provider_id = input.provider_id.trim();
    if (input.channel_id?.trim()) body.channel_id = input.channel_id.trim();

    return this.request("/fetch-channel-models", {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  async listRequestLogs(limit = 50, offset = 0, filters?: RequestLogsFilter): Promise<RequestLogsResponse> {
    const params = new URLSearchParams();
    params.set("limit", String(limit));
    params.set("offset", String(offset));
    if (filters?.model) params.set("model", filters.model);
    if (filters?.status) params.set("status", filters.status);
    if (filters?.api_key_id) params.set("api_key_id", filters.api_key_id);
    if (filters?.username) params.set("username", filters.username);
    if (filters?.search) params.set("search", filters.search);
    if (filters?.time_from) params.set("time_from", filters.time_from);
    if (filters?.time_to) params.set("time_to", filters.time_to);
    return this.request(`/request-logs?${params.toString()}`);
  }

  async getRequestCapture(requestId: string, userId?: string): Promise<RequestCaptureDetail> {
    const params = new URLSearchParams();
    if (userId) params.set("user_id", userId);
    const query = params.toString();
    return this.request(
      `/request-captures/${encodeURIComponent(requestId)}${query ? `?${query}` : ""}`
    );
  }

  async getDashboardAnalytics(buckets = 8, rangeHours = 24): Promise<DashboardAnalytics> {
    const params = new URLSearchParams();
    params.set("buckets", String(buckets));
    params.set("range_hours", String(rangeHours));
    return this.request(`/analytics?${params.toString()}`);
  }

  async getMyLiveUsage(): Promise<UserLiveUsage> {
    return this.request("/me/live-usage");
  }

  async getAdminOverview(): Promise<AdminOverview> {
    return this.request("/admin/overview");
  }

  async testChannel(
    providerId: string,
    channelId: string,
    model?: string,
    stream = true
  ): Promise<ChannelTestResult> {
    return this.request(`/providers/${providerId}/channels/${channelId}/test`, {
      method: "POST",
      body: JSON.stringify({ ...(model ? { model } : {}), stream }),
    });
  }

  async listMarketplaceModels(): Promise<ModelMetadataRecord[]> {
    return this.request("/marketplace/models");
  }
}

export const api = new ApiClient();
