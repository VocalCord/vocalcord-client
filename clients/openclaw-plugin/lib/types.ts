// Plugin-level types shared by the lifecycle, hooks, and helpers.
//
// IMPORTANT: the OpenClaw plugin SDK surface (`OpenclawApi`, hook
// names, runtime helpers like setMcpServer / addCronJob) is
// described here as an interface rather than imported from
// `openclaw/plugin-sdk`. The peer-dep version may differ across
// gateway releases, and the SDK does not yet publish stable type
// declarations for these helpers. Wire-up at install time is via
// duck-typing on `api`; when stable types ship, swap this file
// for direct imports.

export interface PluginConfig {
  apiKey: string;
  apiBase?: string;
  wsBase?: string;
  publicUrl?: string;
  webhookPort?: number;
  webhookPath?: string;
  gatewayBaseUrl?: string;
  hookToken: string;
}

export interface OpenclawLogger {
  info(msg: string, meta?: Record<string, unknown>): void;
  warn(msg: string, meta?: Record<string, unknown>): void;
  error(msg: string, meta?: Record<string, unknown>): void;
  debug?(msg: string, meta?: Record<string, unknown>): void;
}

export interface ToolCallEvent {
  tool: { name: string };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  arguments?: any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  result?: any;
  error?: {
    code?: number | string;
    message?: string;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    data?: any;
  };
}

export interface ToolCallShortCircuit {
  decision: 'short-circuit';
  result: string;
}

export type HookHandlerResult = void | ToolCallShortCircuit;

export interface OpenclawApi {
  config<T>(): T;
  log: OpenclawLogger;
  on(
    name: 'gateway_start' | 'gateway_stop',
    handler: () => Promise<void> | void,
    opts?: { priority?: number },
  ): void;
  on(
    name: 'before_tool_call' | 'after_tool_call',
    handler: (event: ToolCallEvent) => Promise<HookHandlerResult> | HookHandlerResult,
    opts?: { priority?: number },
  ): void;
  /**
   * Runtime helpers (not yet documented in the public plugin SDK).
   * The plugin uses these to register the MCP server and the cron
   * job idempotently. If the gateway version doesn't expose them,
   * the plugin logs a warning and falls back to no-op (the user
   * can still configure manually via `openclaw mcp set` and
   * `openclaw cron add`).
   */
  runtime?: {
    setMcpServer?(name: string, cfg: McpServerConfig): Promise<void>;
    addCronJob?(spec: CronJobSpec): Promise<{ id: string }>;
    listCronJobs?(): Promise<Array<{ id: string; name?: string }>>;
  };
}

export interface McpServerConfig {
  url: string;
  transport: 'streamable-http' | 'stdio';
  headers?: Record<string, string>;
}

export interface CronJobSpec {
  name: string;
  cron: string;
  message: string;
  tools?: string[];
  session?: 'isolated' | 'main';
}

export interface DefinePluginEntryArgs {
  id: string;
  name: string;
  register(api: OpenclawApi): Promise<void> | void;
}
