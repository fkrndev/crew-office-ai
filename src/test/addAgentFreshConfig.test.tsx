/**
 * A fresh config has no `registeredRepos` key at all: opening "Add agent" must
 * not throw (it white-screened the whole renderer before the `?? []` guards).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { AddAgentModal } from '@/components/AddAgentModal';
import type { HarnessConfig } from '@/store/config';
import { useStore } from '@/store/store';

describe('AddAgentModal with a fresh config', () => {
  beforeEach(() => {
    installFakeBridge();
    useStore.setState({ hireQueue: { pending: [], reviewed: 0 } } as never);
  });

  it('renders when config.registeredRepos is absent', () => {
    const config = {
      onboardingComplete: true,
      harnessHome: 'C:/hive',
      autoMode: true,
      defaultCommand: 'claude',
      semanticMemory: false,
      embeddingModel: 'minilm'
    } as unknown as HarnessConfig;
    expect(config.registeredRepos).toBeUndefined();
    render(<AddAgentModal onClose={() => {}} config={config} onConfigChange={vi.fn()} />);
    // The Workspace tab (project quick-picks) renders too.
    fireEvent.click(screen.getByText('Workspace'));
    expect(screen.getByText('add project')).toBeTruthy();
  });
});
