/**
 * QuitWarningModal — the last gate before the floor dies. "force quit" must
 * reach onConfirm (backend tears down every PTY); the backdrop click is
 * suspended while closing-time runs so a stray click can't cancel a shutdown.
 */
import { describe, it, expect } from 'vitest';
import { vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { QuitWarningModal } from '@/components/QuitWarningModal';
import { setGodName } from '@/i18n';

describe('QuitWarningModal', () => {
  it('shows the live count and fires onConfirm on force quit', async () => {
    installFakeBridge();
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(
      <QuitWarningModal ptyCount={3} onCancel={() => {}} onConfirm={onConfirm} />
    );
    expect(screen.getByText(/3 AGENTS STILL RUNNING/)).toBeTruthy();

    fireEvent.click(screen.getByText('kill all & quit'));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
  });

  it('backdrop click cancels only when closing-time is NOT running', () => {
    installFakeBridge();
    const onCancel = vi.fn();
    const { container } = render(
      <QuitWarningModal ptyCount={1} onCancel={onCancel} onConfirm={() => {}} />
    );
    // Backdrop = the fixed overlay div itself.
    fireEvent.click(container.firstChild as Element);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('closing-time phase swaps the copy and keeps force-quit available', async () => {
    installFakeBridge();
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(
      <QuitWarningModal
        ptyCount={2}
        closing={{ phase: 'progress', acked: 1, total: 2 }}
        onCancel={() => {}}
        onConfirm={onConfirm}
      />
    );
    expect(screen.getByText(/WRAPPING UP THE FLOOR/)).toBeTruthy();
    expect(screen.getByText(/1 \/ 2 WORKERS CONFIRMED/)).toBeTruthy();

    fireEvent.click(screen.getByText('force quit now'));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
  });

  it('a rejected force-quit shows the error and re-enables the buttons (never stuck on killing…)', async () => {
    installFakeBridge();
    const onConfirm = vi.fn().mockRejectedValue(new Error('pty teardown failed'));
    render(<QuitWarningModal ptyCount={2} onCancel={() => {}} onConfirm={onConfirm} />);

    fireEvent.click(screen.getByText('kill all & quit'));
    await waitFor(() => expect(screen.getByText(/pty teardown failed/)).toBeTruthy());
    // Back to a pressable button, not a permanent "killing…".
    expect(screen.queryByText('killing…')).toBeNull();
    expect((screen.getByText('kill all & quit').closest('button') as HTMLButtonElement).disabled).toBe(false);
  });

  it('Esc cancels in the warning view, but not while closing-time is running', () => {
    installFakeBridge();
    const onCancel = vi.fn();
    const { rerender } = render(<QuitWarningModal ptyCount={1} onCancel={onCancel} onConfirm={() => {}} />);
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledTimes(1);

    rerender(
      <QuitWarningModal
        ptyCount={1}
        closing={{ phase: 'progress', acked: 0, total: 1 }}
        onCancel={onCancel}
        onConfirm={() => {}}
      />
    );
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('the tip renders the god name as TEXT — markup in a renamed god never becomes elements', async () => {
    installFakeBridge();
    // The live god name reaches the string through i18n's default variables.
    setGodName('<img src=x onerror=alert(1)>Boss');
    const { container } = render(<QuitWarningModal ptyCount={1} onCancel={() => {}} onConfirm={() => {}} />);
    await waitFor(() => expect(container.textContent).toMatch(/Boss/));
    expect(container.querySelector('img')).toBeNull();
    // The legit emphasis from the translation still renders.
    expect(container.querySelector('strong')?.textContent).toBe('closing time');
  });

  it('complete phase removes the action buttons — the app exits on its own', () => {
    installFakeBridge();
    render(
      <QuitWarningModal
        ptyCount={1}
        closing={{ phase: 'complete', acked: 1, total: 1 }}
        onCancel={() => {}}
        onConfirm={() => {}}
      />
    );
    expect(screen.getByText(/FLOOR SAVED/)).toBeTruthy();
    expect(screen.queryByText('force quit now')).toBeNull();
    expect(screen.queryByText('cancel — back to work')).toBeNull();
  });
});
