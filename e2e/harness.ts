import { ALIAS, type Harness } from "./core";
export interface ClientConfig {
  harness: Harness;
  key: string;
  prompt: string;
}
export function launch(config: ClientConfig) {
  const base = "http://recorder:4080";
  const env: Record<string, string> = {
    HOME: "/home/harness",
    PATH: "/suite/node_modules/.bin:/usr/local/bin:/usr/bin:/bin",
    CI: "1",
    NO_COLOR: "1",
    TERM: "dumb",
    E2E_API_KEY: config.key,
  };
  const files: Record<string, string> = {};
  let args: string[];
  if (config.harness.startsWith("codex")) {
    env.CODEX_HOME = "/home/harness/.codex";
    files[env.CODEX_HOME + "/config.toml"] =
      `model = "${ALIAS}"\nmodel_provider = "e2e"\n[features]\nresponses_websockets_v2 = true\n[model_providers.e2e]\nname = "E2E"\nbase_url = "${base}/v1"\nenv_key = "E2E_API_KEY"\nwire_api = "responses"\nsupports_websockets = true\nrequest_max_retries = 0\nstream_max_retries = 0\n`;
    args = [
      "codex",
      "exec",
      "--json",
      "--dangerously-bypass-approvals-and-sandbox",
      "--skip-git-repo-check",
      "--model",
      ALIAS,
      "--cd",
      "/work",
      config.prompt,
    ];
  } else if (config.harness === "claude-messages") {
    Object.assign(env, {
      ANTHROPIC_BASE_URL: base,
      ANTHROPIC_API_KEY: config.key,
      ANTHROPIC_MODEL: ALIAS,
      MAX_THINKING_TOKENS: "0",
      CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING: "1",
      ANTHROPIC_DEFAULT_SONNET_MODEL: ALIAS,
      ANTHROPIC_DEFAULT_OPUS_MODEL: ALIAS,
      ANTHROPIC_DEFAULT_HAIKU_MODEL: ALIAS,
      CLAUDE_CODE_SUBAGENT_MODEL: ALIAS,
      DISABLE_NONESSENTIAL_TRAFFIC: "1",
      CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1",
      CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    });
    args = [
      "claude",
      "-p",
      "--verbose",
      "--output-format",
      "stream-json",
      "--dangerously-skip-permissions",
      "--model",
      ALIAS,
      config.prompt,
    ];
  } else if (config.harness.startsWith("opencode")) {
    Object.assign(env, {
      OPENCODE_DISABLE_AUTOUPDATE: "true",
      OPENCODE_DISABLE_MODELS_FETCH: "true",
      OPENCODE_DISABLE_DEFAULT_PLUGINS: "true",
      OPENCODE_DISABLE_LSP_DOWNLOAD: "true",
      OPENCODE_CONFIG: "/home/harness/opencode.json",
    });
    const type = config.harness.endsWith("-chat") ? "chat" : "responses";
    files[env.OPENCODE_CONFIG!] = JSON.stringify({
      $schema: "https://opencode.ai/config.json",
      model: `e2e/${ALIAS}`,
      small_model: `e2e/${ALIAS}`,
      share: "disabled",
      autoupdate: false,
      enabled_providers: ["e2e"],
      permission: "allow",
      agent: { title: { disable: true }, summary: { disable: true } },
      provider: {
        e2e: {
          npm: `file:///suite/opencode-${type}.js`,
          name: "E2E",
          options: { baseURL: base + "/v1", apiKey: config.key },
          models: {
            [ALIAS]: { name: ALIAS, limit: { context: 128000, output: 8192 } },
          },
        },
      },
    });
    args = [
      "opencode",
      "run",
      "--format",
      "json",
      "--model",
      `e2e/${ALIAS}`,
      config.prompt,
    ];
  } else {
    env.PI_CODING_AGENT_DIR = "/home/harness/.pi/agent";
    files[env.PI_CODING_AGENT_DIR + "/models.json"] = JSON.stringify({
      providers: {
        e2e: {
          baseUrl: base + "/v1",
          api: config.harness.endsWith("-chat")
            ? "openai-completions"
            : "openai-responses",
          apiKey: config.key,
          models: [
            {
              id: ALIAS,
              name: ALIAS,
              reasoning: false,
              input: ["text"],
              contextWindow: 128000,
              maxTokens: 8192,
              cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            },
          ],
        },
      },
    });
    args = [
      "pi",
      "--mode",
      "json",
      "--print",
      "--provider",
      "e2e",
      "--model",
      ALIAS,
      "--no-session",
      "--no-extensions",
      "--no-skills",
      config.prompt,
    ];
  }
  return { args, env, files };
}
export function taskPrompt(repo: string, commit: string, custom?: string) {
  if (custom !== undefined) return custom;
  return `Clone ${repo} into /work/repo with git clone. Check out commit ${commit}. Do not use a different revision. Read source code and explain the architecture. Execute git clone as a separate shell tool call. Read at least three source files using separate shell tool calls, each exactly: cat -- /work/repo/<relative-path>. Do not combine these commands with other commands. Write /work/architecture.md with these Markdown headings: Entry points, Modules, Request flow, Sources. In Sources, cite at least three source files you actually read, using backtick-quoted repository-relative paths. Explain the entry points, main modules, and request data flow briefly. End with a nonempty final response. Prefer small source files. Do not install dependencies or run builds. Do not delegate this task to subagents.`;
}
