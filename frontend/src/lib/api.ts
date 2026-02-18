const API_BASE = "/api/dashboard";

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
}

export interface AuthResponse {
  token: string;
  user: User;
}

export type Phase = "request" | "response";

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
  config_schema: Record<string, unknown>;
}

export interface ApiKey {
  id: string;
  name: string;
  key_prefix: string;
  key: string;
  created_at: string;
  expires_at?: string;
  last_used_at?: string;
  enabled: boolean;
  quota_remaining?: number;
  quota_unlimited: boolean;
  model_limits_enabled: boolean;
  model_limits: string[];
  ip_whitelist: string[];
  group: string;
  max_multiplier?: number;
  transforms: TransformRuleConfig[];
}

export type ApiKeyCreated = ApiKey;

export interface CreateApiKeyInput {
  name: string;
  expires_in_days?: number;
  quota?: number;
  quota_unlimited?: boolean;
  model_limits_enabled?: boolean;
  model_limits?: string[];
  ip_whitelist?: string[];
  group?: string;
  max_multiplier?: number;
  transforms?: TransformRuleConfig[];
}

export interface UpdateApiKeyInput {
  name?: string;
  enabled?: boolean;
  quota?: number;
  quota_unlimited?: boolean;
  model_limits_enabled?: boolean;
  model_limits?: string[];
  ip_whitelist?: string[];
  group?: string;
  max_multiplier?: number;
  transforms?: TransformRuleConfig[];
  expires_at?: string;
}

export interface SystemSettings {
  registration_enabled: boolean;
  default_user_role: string;
  session_ttl_days: number;
  api_key_max_per_user: number;
  site_name: string;
  site_description: string;
  api_base_url: string;
  reasoning_suffix_map: Record<string, string>;
  monoize_active_probe_enabled: boolean;
  monoize_active_probe_interval_seconds: number;
  monoize_active_probe_success_threshold: number;
  monoize_active_probe_model?: string | null;
  updated_at: string;
}

export interface PublicSystemSettings {
  registration_enabled: boolean;
  site_name: string;
  site_description: string;
  api_base_url: string;
}

export interface DashboardStats {
  user_count: number;
  my_api_keys_count: number;
  providers_count: number;
  config_providers_count: number;
  current_user: User;
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
  multiplier: number;
}

export interface MonoizeChannel {
  id: string;
  name: string;
  base_url: string;
  weight: number;
  enabled: boolean;
  _healthy?: boolean;
  _failure_count?: number;
  _last_success_at?: string;
  _health_status?: "healthy" | "probing" | "unhealthy";
}

export interface Provider {
  id: string;
  name: string;
  provider_type: "responses" | "chat_completion" | "messages" | "gemini" | "grok";
  models: Record<string, MonoizeModelEntry>;
  channels: MonoizeChannel[];
  max_retries: number;
  transforms: TransformRuleConfig[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  enabled: boolean;
  priority: number;
  created_at: string;
  updated_at: string;
  unpriced_model_count?: number;
}

export interface CreateMonoizeChannelInput {
  id?: string;
  name: string;
  base_url: string;
  api_key?: string;
  weight?: number;
  enabled?: boolean;
}

export interface CreateProviderInput {
  name: string;
  provider_type: "responses" | "chat_completion" | "messages" | "gemini" | "grok";
  models: Record<string, MonoizeModelEntry>;
  channels: CreateMonoizeChannelInput[];
  max_retries?: number;
  transforms?: TransformRuleConfig[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
  enabled?: boolean;
  priority?: number;
}

export interface UpdateProviderInput {
  name?: string;
  provider_type?: "responses" | "chat_completion" | "messages" | "gemini" | "grok";
  models?: Record<string, MonoizeModelEntry>;
  channels?: CreateMonoizeChannelInput[];
  max_retries?: number;
  transforms?: TransformRuleConfig[];
  active_probe_enabled_override?: boolean | null;
  active_probe_interval_seconds_override?: number | null;
  active_probe_success_threshold_override?: number | null;
  active_probe_model_override?: string | null;
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
  output_cost_per_reasoning_token_nano?: string | null;
  max_input_tokens?: number | null;
  max_output_tokens?: number | null;
  max_tokens?: number | null;
}

export interface ModelMetadataSyncResult {
  success: boolean;
  upserted: number;
  skipped: number;
  fetched_at: string;
}

export interface RequestLog {
  id: string;
  request_id?: string;
  user_id: string;
  api_key_id?: string;
  model: string;
  provider_id?: string;
  upstream_model?: string;
  channel_id?: string;
  channel_name?: string;
  is_stream: boolean;
  prompt_tokens?: number;
  completion_tokens?: number;
  cached_tokens?: number;
  reasoning_tokens?: number;
  provider_multiplier?: number;
  charge_nano_usd?: string;
  status: string;
  usage_breakdown_json?: Record<string, unknown>;
  billing_breakdown_json?: Record<string, unknown>;
  error_code?: string;
  error_message?: string;
  error_http_status?: number;
  tried_providers_json?: Array<{ provider_id: string; channel_id: string; error: string }>;
  duration_ms?: number;
  ttfb_ms?: number;
  request_ip?: string;
  reasoning_effort?: string;
  request_kind?: string;
  created_at: string;
  username?: string;
  api_key_name?: string;
  provider_name?: string;
}

export interface RequestLogsFilter {
  model?: string;
  status?: string;
  api_key_id?: string;
  username?: string;
  search?: string;
}

export interface RequestLogsResponse {
  data: RequestLog[];
  total: number;
  limit: number;
  offset: number;
}

export interface ChannelTestResult {
  success: boolean;
  latency_ms: number;
  model: string;
  error: string | null;
}

class ApiClient {
  private token: string | null = null;

  setToken(token: string | null) {
    this.token = token;
    if (token) {
      localStorage.setItem("token", token);
    } else {
      localStorage.removeItem("token");
    }
  }

  getToken(): string | null {
    if (!this.token) {
      this.token = localStorage.getItem("token");
    }
    return this.token;
  }

  private async request<T>(
    path: string,
    options: RequestInit = {}
  ): Promise<T> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...(options.headers as Record<string, string>),
    };

    const token = this.getToken();
    if (token) {
      headers["Authorization"] = `Bearer ${token}`;
    }

    const response = await fetch(`${API_BASE}${path}`, {
      ...options,
      headers,
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(data.error?.message || data.error?.code || "Request failed");
    }

    return data;
  }

  // Auth
  async register(username: string, password: string): Promise<AuthResponse> {
    return this.request("/auth/register", {
      method: "POST",
      body: JSON.stringify({ username, password }),
    });
  }

  async login(username: string, password: string): Promise<AuthResponse> {
    return this.request("/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password }),
    });
  }

  async logout(): Promise<void> {
    await this.request("/auth/logout", { method: "POST" });
    this.setToken(null);
  }

  async me(): Promise<User> {
    return this.request("/auth/me");
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
    role?: string
  ): Promise<User> {
    return this.request("/users", {
      method: "POST",
      body: JSON.stringify({ username, password, role }),
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
    }
  ): Promise<User> {
    return this.request(`/users/${id}`, {
      method: "PUT",
      body: JSON.stringify(updates),
    });
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

  async syncModelMetadataFromModelsDev(): Promise<ModelMetadataSyncResult> {
    return this.request("/model-metadata/sync/models-dev", {
      method: "POST",
    });
  }

  async fetchProviderModels(providerId: string): Promise<{
    provider_id: string;
    provider_name: string;
    models: string[];
  }> {
    return this.request(`/providers/${providerId}/fetch-models`, {
      method: "POST",
    });
  }

  async fetchChannelModels(baseUrl: string, apiKey: string): Promise<{
    models: string[];
  }> {
    return this.request("/fetch-channel-models", {
      method: "POST",
      body: JSON.stringify({ base_url: baseUrl, api_key: apiKey }),
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
    return this.request(`/request-logs?${params.toString()}`);
  }

  async testChannel(providerId: string, channelId: string, model?: string): Promise<ChannelTestResult> {
    return this.request(`/providers/${providerId}/channels/${channelId}/test`, {
      method: "POST",
      body: model ? JSON.stringify({ model }) : JSON.stringify({}),
    });
  }
}

export const api = new ApiClient();
