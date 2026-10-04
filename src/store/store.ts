// The `@/store/store` contract the upstream office scene reads — a zustand
// store exposing the upstream Agent roster shape. Upstream's real store.ts
// (50KB) also drives terminals/panels/hire queues; this adapter carries ONLY
// the surface the Pixi floor consumes, populated from the Tauri backend:
// `hive_agent_directory` joined with `pty_list`, refreshed on a 2s poll and
// nudged instantly by the emitted lifecycle events.
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AccentColorName } from '@/design/tokens';
import type { OfficeCharacterName } from '@/scene/office/cast';
import { OFFICE_CAST, DEFAULT_CHARACTER } from '@/scene/office/cast';
import type { ThemeId } from '@/scene/office/themeRegistry';

// ─── upstream type surface (store.ts:22) ─────────────────────────────────────

export type ToolKind =
  | 'Read' | 'Edit' | 'Write' | 'Bash' | 'WebFetch' | 'WebSearch'
  | 'Grep' | 'Glob' | 'TodoWrite' | 'MCP';

export type StationKind =
  | 'shelf' | 'terminal' | 'web' | 'board' | 'mailbox' | 'mcp' | 'desk';

export interface BlockReason {
  summary: string;
  detail: string;
  command?: string;
  actions: Array<{ label: string; kind: 'approve' | 'deny' | 'neutral'; send?: string }>;
}

/** PixelBadge StatusKind — the floor's status vocabulary. */
export type StatusKind =
  | 'idle' | 'thinking' | 'working' | 'waiting' | 'blocked' | 'success' | 'ghost'
  | 'compacting' | 'looping' | 'typing';

export type AgentProvider = string;

export interface Agent {
  id: string;
  name: string;
  character: OfficeCharacterName;
  accent: AccentColorName;
  description: string;
  project: string;
  tmuxTarget: string;
  cwd: string;
  goal?: string;
  note?: string;
  status: StatusKind;
  action: string;
  progress: number;
  currentStation?: StationKind;
  carrying?: ToolKind;
  recentAssistantText?: string;
  recentTextTs?: number;
  blockReason?: BlockReason;
  ptyId?: string;
  terminalGeneration?: number;
  command?: string;
  provider?: AgentProvider;
  model?: string;
  lastPrompt?: string;
  isGod?: boolean;
  isAssistant?: boolean;
  onHold?: boolean;
  worktreePath?: string;
  contextTokens?: number;
  contextLimit?: number;
}

// ─── roster shaping ──────────────────────────────────────────────────────────

const ACCENTS: AccentColorName[] = ['coral', 'mint', 'sky', 'lemon', 'lilac', 'peach'];

/** Stable accent/character assignment — a small hash of the id so an agent
 *  keeps its avatar across roster rebuilds (upstream assigns once at spawn). */
function castFor(id: string): OfficeCharacterName {
  if (id === 'god') return 'michael';
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  // Index 0 is michael — the orchestrator's seat is already taken.
  return OFFICE_CAST[(h % (OFFICE_CAST.length - 1)) + 1].name ?? DEFAULT_CHARACTER;
}

function accentFor(id: string): AccentColorName {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 131 + id.charCodeAt(i)) >>> 0;
  return ACCENTS[h % ACCENTS.length];
}

const TOOL_STATION: Record<string, StationKind> = {
  Read: 'shelf', Edit: 'desk', Write: 'desk', Bash: 'terminal',
  WebFetch: 'web', WebSearch: 'web', Grep: 'terminal', Glob: 'terminal',
  TodoWrite: 'board', MCP: 'mcp'
};

function toolKind(tool?: string): ToolKind | undefined {
  if (!tool) return undefined;
  const t = tool.replace(/^mcp__.*$/, 'MCP');
  return (Object.keys(TOOL_STATION) as ToolKind[]).find((k) => t === k)
    ?? (t === 'MCP' ? 'MCP' : undefined);
}

// ─── store ───────────────────────────────────────────────────────────────────

interface OfficeState {
  agents: Agent[];
  selectedId: string | null;
  officeTheme: ThemeId;
  fullscreenAgentId: string | null;
  ideOpen: boolean;
  /** Upstream opens the god's Command Center tab; the port selects god and
   *  emits a DOM event the app shell can map onto its own panel. */
  requestCommandCenterTab: (tab: string) => void;
  select: (id: string | null) => void;
  selectAgent: (id: string | null) => void;
  setOfficeTheme: (t: ThemeId) => void;
}

/** registry `status` + breaker level → the floor's vocabulary. A `gone`/
 *  archived agent is a ghost; a breaker past 'warning' reads as 'looping'. */
function floorStatus(row: Record<string, unknown>, live: boolean): StatusKind {
  if (row.archived === true) return 'ghost';
  const s = String(row.status ?? 'idle');
  if (s === 'gone' || s === 'stopped') return 'ghost';
  if (s === 'blocked') return 'blocked';
  if (!live) return 'ghost';
  const brk = row.breaker;
  const level = typeof brk === 'number' ? brk : typeof brk === 'string' ? brk : '';
  if (level === 2 || level === 3 || level === 'armed' || level === 'tripped') return 'looping';
  if (s === 'working') return 'working';
  return 'idle';
}

export const useStore = create<OfficeState>()((set, get) => ({
  agents: [],
  selectedId: null,
  officeTheme: 'office',
  fullscreenAgentId: null,
  ideOpen: false,
  requestCommandCenterTab: (tab) => {
    const god = get().agents.find((a) => a.isGod);
    if (god) set({ selectedId: god.id });
    window.dispatchEvent(new CustomEvent('ao:command-center-tab', { detail: { tab } }));
  },
  select: (id) => {
    set({ selectedId: id });
    window.dispatchEvent(new CustomEvent('ao:select-agent', { detail: { id } }));
  },
  selectAgent: (id) => get().select(id),
  setOfficeTheme: (t) => set({ officeTheme: t })
}));

// ─── backend sync ────────────────────────────────────────────────────────────

interface DirectoryRow {
  id: string; name: string; role?: string; provider?: string; model?: string | null;
  status?: string; cwd?: string; cwdValid?: boolean; archived?: boolean;
  isGod?: boolean; isAssistant?: boolean; onHold?: boolean; sessionId?: string;
  breaker?: unknown; tokens?: number; usd?: number; lastTool?: string | null;
  lastActiveSecAgo?: number; contextTokens?: number; contextLimit?: number;
  command?: string;
}

async function syncRoster(): Promise<void> {
  const dir = await invoke<{ godId?: string; agents?: DirectoryRow[] }>('hive_agent_directory')
    .catch(() => null);
  const ptys = await invoke<Array<{ id: string; command?: string }>>('pty_list')
    .catch(() => [] as Array<{ id: string; command?: string }>);
  const live = new Set(ptys.map((p) => p.id));
  const ptyById = new Map(ptys.map((p) => [p.id, p]));
  const prev = new Map(useStore.getState().agents.map((a) => [a.id, a]));

  const agents: Agent[] = (dir?.agents ?? []).map((r) => {
    const p = prev.get(r.id);
    const st = floorStatus(r as unknown as Record<string, unknown>, live.has(r.id));
    const tool = toolKind(r.lastTool ?? undefined);
    return {
      id: r.id,
      name: r.name,
      character: r.isGod ? 'michael' : castFor(r.id),
      accent: accentFor(r.id),
      description: r.role ?? (r.isGod ? 'orchestrator' : 'agent'),
      project: r.cwd ?? '',
      tmuxTarget: '',
      cwd: r.cwd ?? '',
      status: p?.status === 'compacting' || p?.status === 'success' ? p.status : st,
      action: r.lastTool ?? (st === 'idle' ? 'idling' : st),
      progress: 0,
      currentStation: tool ? TOOL_STATION[tool] : 'desk',
      carrying: tool,
      ptyId: live.has(r.id) ? r.id : undefined,
      command: ptyById.get(r.id)?.command,
      provider: r.provider,
      model: r.model ?? undefined,
      lastPrompt: p?.lastPrompt,
      isGod: r.isGod,
      isAssistant: r.isAssistant,
      onHold: r.onHold,
      worktreePath: live.has(r.id) ? r.cwd : undefined,
      contextTokens: r.contextTokens,
      contextLimit: r.contextLimit
    };
  });
  useStore.setState({ agents });
}

/** Apply one live event to the floor roster without waiting for the poll. */
function patchAgent(id: string, patch: Partial<Agent>): void {
  const agents = useStore.getState().agents;
  const i = agents.findIndex((a) => a.id === id);
  if (i < 0) return;
  const next = agents.slice();
  next[i] = { ...agents[i], ...patch };
  useStore.setState({ agents: next });
}

let installed = false;

/** Wire the floor roster to the backend — call once from main.tsx. */
export function installOfficeStore(): void {
  if (installed) return;
  installed = true;
  void syncRoster();
  window.setInterval(() => void syncRoster(), 2000);

  // Hook stream → live floor status: a tool call means working + the carried
  // tool; a notification is a blocked-ish waiting state; prompts surface as
  // the card above the avatar; compact hooks drive the compacting state.
  void listen<{ agentId?: string; event?: string; tool?: string; message?: string; notificationType?: string }>(
    'hook-event',
    (e) => {
      const id = e.payload.agentId;
      if (!id) return;
      const tool = toolKind(e.payload.tool);
      if (e.payload.event === 'PreToolUse' && tool) {
        patchAgent(id, { status: 'working', action: e.payload.tool!, carrying: tool, currentStation: TOOL_STATION[tool] });
      } else if (e.payload.event === 'UserPromptSubmit' || e.payload.event === 'SessionStart') {
        patchAgent(id, { status: 'thinking', lastPrompt: e.payload.message ?? undefined });
      } else if (e.payload.event === 'Notification') {
        patchAgent(id, { status: 'waiting' });
      } else if (e.payload.event === 'PreCompact') {
        patchAgent(id, { status: 'compacting', action: 'compacting' });
      } else if (e.payload.event === 'PostCompact') {
        patchAgent(id, { status: 'working' });
      } else if (e.payload.event === 'Stop') {
        patchAgent(id, { status: 'success', action: 'done', carrying: undefined });
      }
    }
  );
  void listen<{ id?: string; status?: string }>('agent-status', (e) => {
    if (!e.payload.id) return;
    patchAgent(e.payload.id, {
      status: e.payload.status === 'stopped' || e.payload.status === 'gone' ? 'ghost' : 'idle'
    });
  });
  void listen('pty-exit', () => void syncRoster());
  void listen('hive-agent-spawned', () => void syncRoster());
  void listen('hive-agent-archived', (e) => {
    const id = (e.payload as { id?: string }).id;
    if (id) patchAgent(id, { status: 'ghost' });
  });
}
