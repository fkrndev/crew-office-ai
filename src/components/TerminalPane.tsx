import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { onPtyOutput, ptyResize, ptyWrite } from '../api';

/** Live PTY view — xterm.js bound to a backend PTY via pty-output events and
 *  pty_write commands (replaces the Electron ipcRenderer channel pair). */
export function TerminalPane({ agentId }: { agentId: string }) {
  const host = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    const term = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontFamily: 'Cascadia Mono, Consolas, monospace',
      fontSize: 13,
      theme: { background: '#1a1a1e' }
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current!);
    fit.fit();
    termRef.current = term;

    const unOut = onPtyOutput((p) => {
      if (p.id === agentId) term.write(p.data);
    });
    const sub = term.onData((d) => ptyWrite(agentId, d));
    const ro = new ResizeObserver(() => {
      fit.fit();
      ptyResize(agentId, term.cols, term.rows).catch(() => {});
    });
    ro.observe(host.current!);

    return () => {
      ro.disconnect();
      sub.dispose();
      unOut.then((f) => f());
      term.dispose();
    };
  }, [agentId]);

  return <div ref={host} className="terminal-host" />;
}
