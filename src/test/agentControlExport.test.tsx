/**
 * AgentControlStrip → export button E2E — the whole "export session" flow
 * against a FakeBridge: chooseFolder → gather (usage + mail + pty tail) →
 * writeFile → revealPath. Asserts every leg is called and the file body is
 * the composed Markdown.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { AgentControlStrip } from '@/components/AgentControlStrip';
import { installFakeBridge } from './fakeBridge';
import { useStore } from '@/store/store';

const AGENT_ID = 'a1';

function seedAgent() {
  const st = useStore.getState();
  useStore.setState({
    ...st,
    agents: [
      {
        id: AGENT_ID,
        name: 'Ada',
        provider: 'claude',
        ptyId: 'pty-1',
        cwd: 'D:\\repo'
        // store Agent type has more fields; the export only reads these.
      } as never
    ]
  });
}

describe('AgentControlStrip export', () => {
  it('writes a session Markdown to the picked folder and reveals it', async () => {
    seedAgent();
    const writeFile = vi.fn(async () => ({ ok: true, path: 'C:\\out\\Ada-session.md' }));
    const revealPath = vi.fn(async () => true);
    const chooseFolder = vi.fn(async () => ({ ok: true, path: 'C:\\out' }));
    const telemetryUsage = vi.fn(async () => ({ input: 10, output: 5, usd: 0.01 }));
    const hiveMessages = vi.fn(async () => [
      { from: 'god', to: 'a1', subject: 'hi', body: 'b', direction: 'inbox', created_at: 't' }
    ]);
    const redrawPty = vi.fn(async () => 'plain tail text');
    installFakeBridge({
      controlSnapshot: async () => ({ paused: false, halted: false, autoDeliveryPaused: false, gatedTools: [], pendingSteers: 0 }),
      chooseFolder, writeFile, revealPath, telemetryUsage, hiveMessages, redrawPty
    });

    render(<AgentControlStrip agentId={AGENT_ID} />);
    fireEvent.click(screen.getByRole('button', { name: /export this agent/i }));

    await waitFor(() => expect(revealPath).toHaveBeenCalled());
    expect(chooseFolder).toHaveBeenCalledTimes(1);
    expect(telemetryUsage).toHaveBeenCalledWith(AGENT_ID);
    expect(hiveMessages).toHaveBeenCalled();
    expect(redrawPty).toHaveBeenCalledWith('pty-1');
    const [folder, fileName, body] = writeFile.mock.calls[0] as unknown as [string, string, string];
    expect(folder).toBe('C:\\out');
    expect(fileName).toMatch(/^Ada-session-.*\.md$/);
    expect(body).toContain('# Session export — Ada');
    expect(body).toContain('plain tail text');
    expect(revealPath).toHaveBeenCalledWith('C:\\out\\Ada-session.md');
  });

  it('a cancelled folder pick writes nothing', async () => {
    seedAgent();
    const writeFile = vi.fn();
    installFakeBridge({
      controlSnapshot: async () => null,
      chooseFolder: async () => ({ ok: false, error: 'cancelled' }),
      writeFile
    });
    render(<AgentControlStrip agentId={AGENT_ID} />);
    fireEvent.click(screen.getByRole('button', { name: /export this agent/i }));
    await waitFor(() => expect(writeFile).not.toHaveBeenCalled());
  });
});
