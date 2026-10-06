/**
 * Project (registeredRepos) CRUD â€” the AddAgentModal quick-pick row.
 * Renders the real modal against fakeBridge and drives the two real actions:
 * "add project" (chooseFolder â†’ registerProject) and the per-chip Ã— remove.
 *
 * Note: fakeBridge only records calls that hit the DEFAULT stub â€” overrides
 * return their own function verbatim, so these tests capture patches through
 * the override itself.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { AddAgentModal } from '@/components/AddAgentModal';
import type { HarnessConfig } from '@/store/config';
import { useStore } from '@/store/store';

const baseConfig: HarnessConfig = {
  onboardingComplete: true,
  harnessHome: 'C:/hive',
  registeredRepos: ['C:/repo-a', 'C:/repo-b'],
  autoMode: true,
  defaultCommand: 'claude',
  semanticMemory: false,
  embeddingModel: 'minilm'
};

function renderModal(config: HarnessConfig = baseConfig) {
  const onConfigChange = vi.fn();
  render(<AddAgentModal onClose={() => {}} config={config} onConfigChange={onConfigChange} />);
  // The project picker lives on the Workspace tab.
  fireEvent.click(screen.getByText('Workspace'));
  return { onConfigChange };
}

/** Captured updateConfig patches + the config the modal adopts back. */
function captureConfig() {
  const patches: Array<{ registeredRepos?: string[] }> = [];
  const updateConfig = async (patch: { registeredRepos?: string[] }) => {
    patches.push(patch);
    return patch;
  };
  return { patches, updateConfig };
}

describe('AddAgentModal project CRUD', () => {
  let cth: FakeBridge;
  beforeEach(() => {
    cth = installFakeBridge();
    useStore.setState({ hireQueue: { pending: [], reviewed: 0 } } as never);
  });

  it('add project: chooseFolder + updateConfig dedupe-prepends the new repo', async () => {
    const { patches, updateConfig } = captureConfig();
    cth.chooseFolder = async () => ({ ok: true, path: 'C:/new-proj' });
    cth.updateConfig = updateConfig;
    const { onConfigChange } = renderModal();

    fireEvent.click(screen.getByText('add project'));

    await waitFor(() => {
      expect(patches.some(
        (p) => JSON.stringify(p.registeredRepos) === JSON.stringify(['C:/new-proj', 'C:/repo-a', 'C:/repo-b'])
      )).toBe(true);
    });
    // The lifted config makes the new pick stick for the next modal open.
    expect(onConfigChange).toHaveBeenCalled();
  });

  it('re-registering an existing repo moves it to the front without duplicating', async () => {
    const { patches, updateConfig } = captureConfig();
    cth.chooseFolder = async () => ({ ok: true, path: 'C:/repo-b' });
    cth.updateConfig = updateConfig;
    renderModal();

    fireEvent.click(screen.getByText('add project'));

    await waitFor(() => {
      expect(patches.some(
        (p) => JSON.stringify(p.registeredRepos) === JSON.stringify(['C:/repo-b', 'C:/repo-a'])
      )).toBe(true);
    });
  });

  it('cancelled folder pick never touches config', async () => {
    const { patches, updateConfig } = captureConfig();
    let choseFolder = 0;
    cth.chooseFolder = async () => { choseFolder++; return { ok: false, error: 'cancelled' }; };
    cth.updateConfig = updateConfig;
    renderModal();

    fireEvent.click(screen.getByText('add project'));

    await waitFor(() => expect(choseFolder).toBe(1));
    expect(patches).toHaveLength(0);
  });

  it('remove chip drops the repo from the list and persists', async () => {
    const { patches, updateConfig } = captureConfig();
    cth.updateConfig = updateConfig;
    const { onConfigChange } = renderModal();

    fireEvent.click(screen.getByLabelText('Remove repo-a from the project list'));

    await waitFor(() => {
      expect(patches.some(
        (p) => JSON.stringify(p.registeredRepos) === JSON.stringify(['C:/repo-b'])
      )).toBe(true);
    });
    expect(onConfigChange).toHaveBeenCalled();
    // The chip is gone from the picker.
    expect(screen.queryByTitle('C:/repo-a')).toBeNull();
    expect(screen.getByTitle('C:/repo-b')).toBeTruthy();
  });
});
