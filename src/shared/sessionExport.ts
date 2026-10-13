/**
 * Session export composer — one agent's session as a single Markdown file:
 * identity, spend, hive mail, and the terminal scrollback tail. Pure function
 * so the export shape is testable without a renderer.
 *
 * Inputs come from the renderer's existing seams: the store's Agent record,
 * `telemetry_usage`, `hive_messages` (already secrets-redacted main-side), and
 * `pty_redraw` (ANSI-stripped by the caller). Nothing here touches the fs.
 */

export interface SessionExportAgent {
  id: string;
  name: string;
  provider?: string;
  model?: string;
  cwd?: string;
  goal?: string;
  description?: string;
}

export interface SessionExportUsage {
  input?: number;
  output?: number;
  cacheRead?: number;
  cacheCreation?: number;
  usd?: number;
}

export interface SessionExportMessage {
  id?: string;
  from?: unknown;
  to?: unknown;
  act?: string;
  subject?: string;
  body?: string;
  direction?: string;
  archived?: boolean;
  created_at?: unknown;
}

const FIELD = (v: unknown): string => (typeof v === 'string' ? v : '');
const ACTOR = (v: unknown): string =>
  typeof v === 'string' ? v : typeof v === 'object' && v !== null
    ? FIELD((v as Record<string, unknown>).id) || FIELD((v as Record<string, unknown>).name) || '?'
    : '?';

const TAIL_MAX_LINES = 400;

export function buildSessionExport(
  agent: SessionExportAgent | undefined,
  usage: SessionExportUsage | null,
  messages: SessionExportMessage[],
  terminalTail: string,
  exportedAt = new Date()
): string {
  const name = agent?.name ?? 'unknown-agent';
  const out: string[] = [];
  out.push(`# Session export — ${name}`);
  out.push('');
  out.push(`- Agent: \`${agent?.id ?? '?'}\``);
  if (agent?.provider) out.push(`- Provider: ${agent.provider}${agent?.model ? ` · ${agent.model}` : ''}`);
  if (agent?.description) out.push(`- Role: ${agent.description}`);
  if (agent?.goal) out.push(`- Goal: ${agent.goal}`);
  if (agent?.cwd) out.push(`- Workspace: \`${agent.cwd}\``);
  out.push(`- Exported: ${exportedAt.toISOString()}`);

  if (usage) {
    out.push('');
    out.push('## Usage');
    out.push('');
    const tok = (usage.input ?? 0) + (usage.output ?? 0) + (usage.cacheRead ?? 0) + (usage.cacheCreation ?? 0);
    out.push(`- Total tokens: ${tok.toLocaleString()}`);
    out.push(`- Input: ${(usage.input ?? 0).toLocaleString()} · Output: ${(usage.output ?? 0).toLocaleString()} · Cache: ${((usage.cacheRead ?? 0) + (usage.cacheCreation ?? 0)).toLocaleString()}`);
    if (usage.usd != null) out.push(`- Spend: $${usage.usd.toFixed(2)}`);
  }

  if (messages.length > 0) {
    out.push('');
    out.push(`## Inbox (${messages.length})`);
    out.push('');
    for (const m of [...messages].reverse()) { // newest-first → chronological
      const when = typeof m.created_at === 'string' ? m.created_at : '';
      const dir = m.direction === 'outbox' ? '→' : '←';
      out.push(`### ${when} ${dir} ${ACTOR(m.from)} → ${ACTOR(m.to)}`);
      if (m.subject) out.push(`**${m.subject}**`);
      if (m.body) out.push(m.body);
      out.push('');
    }
  }

  const tail = terminalTail.trim();
  if (tail) {
    out.push('## Recent terminal output');
    out.push('');
    out.push('```');
    const lines = tail.split('\n');
    out.push(lines.length > TAIL_MAX_LINES ? lines.slice(-TAIL_MAX_LINES).join('\n') : tail);
    out.push('```');
  }

  return out.join('\n').replace(/\n{3,}/g, '\n\n') + '\n';
}

/** Filesystem-safe slug for the export filename. */
export function sessionExportFileName(name: string, at = new Date()): string {
  const slug = name.replace(/[^\w.-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40) || 'agent';
  const stamp = at.toISOString().replace(/[:T]/g, '-').slice(0, 16);
  return `${slug}-session-${stamp}.md`;
}
