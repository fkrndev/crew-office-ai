/**
 * Pasting a screenshot into the message composer: the PNG has no path, so the
 * bridge writes it to a temp file (clipboardImageToFile) and the returned path
 * is attached exactly like a dropped/picked file. A rejection surfaces the
 * existing notice; plain-text paste is left to the browser.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { MessageQueueComposer } from '@/components/MessageQueueComposer';
import type { Agent } from '@/store/store';

let cth: FakeBridge;
const agent = { id: 'a1', name: 'Dana', status: 'idle' } as unknown as Agent;

function pasteData(over: Record<string, unknown>) {
  return {
    clipboardData: { items: [], files: [], getData: () => '', ...over }
  };
}

function imagePaste() {
  const blob = new File([new Uint8Array([137, 80, 78, 71])], 'image.png', { type: 'image/png' });
  return pasteData({
    items: [{ kind: 'file', type: 'image/png', getAsFile: () => blob }]
  });
}

beforeEach(() => {
  cth = installFakeBridge({
    pathForFile: () => '',
    controlSnapshot: async () => ({})
  });
});

describe('composer screenshot paste', () => {
  it('writes the pasted image to a file and attaches the returned path', async () => {
    cth.clipboardImageToFile = vi.fn(async () => ({ ok: true, path: 'C:\\home\\tmp\\paste-1.png' }));
    render(<MessageQueueComposer agent={agent} />);
    const box = screen.getByRole('textbox');

    fireEvent.paste(box, imagePaste());

    await waitFor(() => expect(screen.getByText('paste-1.png')).toBeTruthy());
    expect(screen.getByTitle('C:\\home\\tmp\\paste-1.png')).toBeTruthy();
    const arg = (cth.clipboardImageToFile as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(arg.startsWith('data:image/png;base64,')).toBe(true);
  });

  it('shows the paste-failed notice when the command rejects', async () => {
    cth.clipboardImageToFile = vi.fn(async () => ({ ok: false, error: 'disk full' }));
    const notices: string[] = [];
    const onNotice = (e: Event) => notices.push((e as CustomEvent).detail.text);
    window.addEventListener('cth:notice', onNotice);
    render(<MessageQueueComposer agent={agent} />);

    fireEvent.paste(screen.getByRole('textbox'), imagePaste());

    await waitFor(() => expect(notices.length).toBe(1));
    expect(notices[0]).toContain('disk full');
    expect(screen.queryByTitle(/paste-1/)).toBeNull();
    window.removeEventListener('cth:notice', onNotice);
  });

  it('leaves plain text paste alone', () => {
    cth.clipboardImageToFile = vi.fn();
    render(<MessageQueueComposer agent={agent} />);
    const notPrevented = fireEvent.paste(
      screen.getByRole('textbox'),
      pasteData({ getData: () => 'hello' })
    );
    expect(notPrevented).toBe(true);
    expect(cth.clipboardImageToFile).not.toHaveBeenCalled();
  });
});
