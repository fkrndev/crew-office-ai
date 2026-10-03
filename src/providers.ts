/**
 * Port of `src/shared/agentProvider.ts` — the preset DATA only.
 * Bridge installation (hook shims / loopback proxies) is a Rust-side concern
 * and not ported yet (see PORTING.md); the `bridge` field is preserved so the
 * information isn't lost.
 */

export type BridgeDescriptor =
  | { kind: 'hooks'; shim: 'agy' | 'codex' | 'pi' | 'opencode' | 'grok' | 'gemini' }
  | {
      kind: 'proxy';
      api: 'openai' | 'anthropic';
      baseUrlEnv: string;
      inboxDelivery: 'terminal' | 'serve';
    };

export interface AgentProviderPreset {
  id: AgentProvider;
  label: string;
  defaultCommand: string;
  autoModeFlag: string;
  supportsModel: boolean;
  modelFlag?: string;
  hiveAware: boolean;
  hookBridge?: 'agy' | 'codex' | 'grok';
  bridge?: BridgeDescriptor;
  recommendedOrchestratorModel?: string;
  canReceiveInbox: boolean;
  initialPromptFlag?: string;
  seedDelivery?: 'type-into-tui';
  positionalInitialPrompt?: boolean;
  resumeFlag?: string;
  nonInteractiveEnv?: Record<string, string>;
  installCommand?: string;
  nativeInstallCommand?: { posix: string; win32: string };
  docsUrl?: string;
  autoStanceTokens?: string[];
  resumeSubcommand?: string;
}

export type AgentProvider =
  | 'claude'
  | 'codex'
  | 'grok'
  | 'kimi'
  | 'gemini'
  | 'antigravity'
  | 'qwen'
  | 'opencode'
  | 'crush'
  | 'pi'
  | 'copilot'
  | 'cursor'
  | 'hermes'
  | 'custom';

export const AGENT_PROVIDER_PRESETS: AgentProviderPreset[] = [
  {
    id: 'claude',
    label: 'Claude Code',
    defaultCommand: 'claude',
    autoModeFlag: '--permission-mode bypassPermissions',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: true,
    canReceiveInbox: true,
    recommendedOrchestratorModel: 'claude-opus-4-8[1m]',
    resumeFlag: '--resume',
    installCommand: 'npm install -g @anthropic-ai/claude-code',
    nativeInstallCommand: {
      posix: 'curl -fsSL https://claude.ai/install.sh | bash',
      win32: 'powershell -c irm https://claude.ai/install.ps1 ^| iex'
    },
    docsUrl: 'https://docs.claude.com/en/docs/claude-code'
  },
  {
    id: 'codex',
    label: 'Codex · GPT',
    defaultCommand: 'codex',
    autoModeFlag: '-a never -s workspace-write',
    autoStanceTokens: ['-a', '--ask-for-approval', '-s', '--sandbox', '--full-auto', '--dangerously-bypass-approvals-and-sandbox'],
    nonInteractiveEnv: { CODEX_NON_INTERACTIVE: '1' },
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    hookBridge: 'codex',
    canReceiveInbox: true,
    positionalInitialPrompt: true,
    recommendedOrchestratorModel: 'gpt-5-codex',
    resumeSubcommand: 'resume',
    installCommand: 'npm install -g @openai/codex',
    docsUrl: 'https://github.com/openai/codex'
  },
  {
    id: 'grok',
    label: 'Grok · xAI',
    defaultCommand: 'grok',
    autoModeFlag: '--permission-mode bypassPermissions',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    hookBridge: 'grok',
    canReceiveInbox: true,
    positionalInitialPrompt: true,
    resumeFlag: '--resume'
  },
  {
    id: 'kimi',
    label: 'Kimi Code',
    defaultCommand: 'kimi',
    autoModeFlag: '--auto',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    canReceiveInbox: false
  },
  {
    id: 'gemini',
    label: 'Gemini CLI',
    defaultCommand: 'gemini',
    autoModeFlag: '--approval-mode=yolo',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    bridge: { kind: 'hooks', shim: 'gemini' },
    canReceiveInbox: true,
    initialPromptFlag: '-i',
    recommendedOrchestratorModel: 'pro',
    resumeFlag: '--resume',
    installCommand: 'npm install -g @google/gemini-cli',
    docsUrl: 'https://github.com/google-gemini/gemini-cli'
  },
  {
    id: 'antigravity',
    label: 'Antigravity · Gemini',
    defaultCommand: 'agy',
    autoModeFlag: '--dangerously-skip-permissions',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    hookBridge: 'agy',
    canReceiveInbox: true,
    initialPromptFlag: '-i',
    recommendedOrchestratorModel: 'Gemini 3.1 Pro (High)',
    resumeFlag: '--conversation'
  },
  {
    id: 'qwen',
    label: 'Qwen (local available)',
    defaultCommand: 'qwen',
    autoModeFlag: '--yolo',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    bridge: { kind: 'proxy', api: 'openai', baseUrlEnv: 'OPENAI_BASE_URL', inboxDelivery: 'terminal' },
    canReceiveInbox: true,
    initialPromptFlag: '-i',
    recommendedOrchestratorModel: 'qwen3-coder-plus'
  },
  {
    id: 'opencode',
    label: 'OpenCode',
    defaultCommand: 'opencode',
    autoModeFlag: '',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    bridge: { kind: 'hooks', shim: 'opencode' },
    canReceiveInbox: true,
    initialPromptFlag: '--prompt',
    installCommand: 'npm install -g opencode-ai@latest',
    nativeInstallCommand: {
      posix: 'curl -fsSL https://opencode.ai/install | bash',
      win32: 'choco install opencode -y'
    },
    docsUrl: 'https://opencode.ai/docs'
  },
  {
    id: 'crush',
    label: 'Crush · Charm',
    defaultCommand: 'crush',
    autoModeFlag: '--yolo',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    bridge: { kind: 'proxy', api: 'openai', baseUrlEnv: 'CRUSH_PROXY_BASE_URL', inboxDelivery: 'terminal' },
    recommendedOrchestratorModel: 'openai/gpt-4o',
    canReceiveInbox: true,
    seedDelivery: 'type-into-tui',
    resumeFlag: '--session',
    installCommand: 'npm install -g @charmland/crush',
    docsUrl: 'https://github.com/charmbracelet/crush'
  },
  {
    id: 'pi',
    label: 'Pi',
    defaultCommand: 'pi',
    autoModeFlag: '--approve',
    nonInteractiveEnv: { PI_SKIP_VERSION_CHECK: '1', PI_TELEMETRY: '0' },
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    bridge: { kind: 'hooks', shim: 'pi' },
    recommendedOrchestratorModel: 'anthropic/claude-sonnet-4-5',
    canReceiveInbox: true,
    positionalInitialPrompt: true,
    resumeFlag: '--session',
    installCommand: 'npm install -g --ignore-scripts @earendil-works/pi-coding-agent',
    docsUrl: 'https://pi.dev/docs/latest'
  },
  {
    id: 'copilot',
    label: 'Copilot',
    defaultCommand: 'copilot',
    autoModeFlag: '-s --allow-all-tools --no-ask-user',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    initialPromptFlag: '-p',
    recommendedOrchestratorModel: 'claude-sonnet-4.5',
    resumeFlag: '--resume',
    canReceiveInbox: false,
    installCommand: 'npm install -g @github/copilot',
    docsUrl: 'https://docs.github.com/copilot/concepts/agents/about-copilot-cli'
  },
  {
    id: 'cursor',
    label: 'Cursor',
    defaultCommand: 'cursor-agent',
    autoModeFlag: '--force --trust',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    canReceiveInbox: true,
    seedDelivery: 'type-into-tui',
    recommendedOrchestratorModel: 'gpt-5.6-luna-high',
    resumeFlag: '--resume',
    nativeInstallCommand: {
      posix: 'curl https://cursor.com/install -fsS | bash',
      win32: 'irm https://cursor.com/install?win32=true | iex'
    },
    docsUrl: 'https://cursor.com/docs/cli/install'
  },
  {
    // Hermes (NousResearch/hermes-agent) — a Python agent runtime, not a
    // hook-bridge CLI: it lands as a 'type-into-tui' peer like crush/cursor,
    // receiving the hive prompt as its typed first turn. `--cli` forces the
    // REPL (the TUI would fight our PTY chrome); inbox rides the typed channel
    // since Hermes has no hook protocol to bridge.
    id: 'hermes',
    label: 'Hermes · Nous',
    defaultCommand: 'hermes --cli',
    autoModeFlag: '',
    supportsModel: true,
    modelFlag: '--model',
    hiveAware: false,
    canReceiveInbox: true,
    seedDelivery: 'type-into-tui',
    installCommand: 'pip install hermes-agent',
    docsUrl: 'https://github.com/NousResearch/hermes-agent'
  },
  {
    id: 'custom',
    label: 'Custom',
    defaultCommand: '',
    autoModeFlag: '',
    supportsModel: false,
    hiveAware: false,
    canReceiveInbox: false
  }
];

export function providerPreset(provider: AgentProvider): AgentProviderPreset {
  return AGENT_PROVIDER_PRESETS.find((p) => p.id === provider) ?? AGENT_PROVIDER_PRESETS[0];
}

/** Assemble the spawn command line the same way the Electron harness did:
 *  default/custom command + auto flag (when the floor is in auto mode and the
 *  command doesn't already carry an explicit stance) + model flag. */
export function buildSpawnCommand(
  provider: AgentProvider,
  customCommand: string,
  autoMode: boolean,
  model?: string
): string {
  const preset = providerPreset(provider);
  const base = customCommand.trim() || preset.defaultCommand;
  let cmd = base;
  if (autoMode && preset.autoModeFlag) {
    const stance = preset.autoStanceTokens ?? [preset.autoModeFlag.split(' ')[0]];
    if (!stance.some((t) => base.includes(t))) cmd += ` ${preset.autoModeFlag}`;
  }
  if (model && preset.supportsModel && preset.modelFlag) {
    cmd += ` ${preset.modelFlag} ${model}`;
  }
  return cmd;
}
