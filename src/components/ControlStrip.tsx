import { useEffect, useState } from 'react';
import {
  controlHalt,
  controlPause,
  controlResume,
  controlSnapshot,
  controlSteer,
  type ControlSnapshot
} from '../api';
import { useApp } from '../store';

/** Operator control strip — control.ts IPC surface: pause, halt (graceful
 *  stop at the next hook boundary), steer (guidance injected as
 *  additionalContext on the next hook). */
export function ControlStrip({ agentId }: { agentId: string }) {
  const [snap, setSnap] = useState<ControlSnapshot | null>(null);
  const [steerText, setSteerText] = useState('');
  const breaker = useApp((s) => s.breaker[agentId]);

  const refresh = () => controlSnapshot(agentId).then(setSnap);
  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  return (
    <div className="control-strip">
      {breaker && breaker.level !== 'healthy' && (
        <span className={`tag breaker ${breaker.level}`} title={breaker.reason}>
          breaker: {breaker.level}
        </span>
      )}
      {snap?.paused && <span className="tag">paused</span>}
      {snap?.halted && <span className="tag">halting…</span>}
      {snap && snap.pendingSteers > 0 && <span className="tag">{snap.pendingSteers} steer(s) queued</span>}
      {snap?.gatedTools?.map((t) => (
        <span key={t} className="tag">
          gated: {t}
        </span>
      ))}
      <div className="grow" />
      <input
        className="steer-input"
        placeholder="steer: guidance injected on next hook…"
        value={steerText}
        onChange={(e) => setSteerText(e.target.value)}
        onKeyDown={async (e) => {
          if (e.key === 'Enter' && steerText.trim()) {
            setSnap(await controlSteer(agentId, steerText));
            setSteerText('');
          }
        }}
      />
      <button
        title={snap?.paused ? 'Resume (clears pause + halt)' : 'Pause — denies every tool call'}
        onClick={async () => setSnap(snap?.paused ? await controlResume(agentId) : await controlPause(agentId, true))}
      >
        {snap?.paused ? '▶ resume' : '⏸ pause'}
      </button>
      <button
        className="danger ghost"
        title="Halt — stop cleanly at the next hook boundary (not a PTY kill)"
        onClick={async () => setSnap(await controlHalt(agentId))}
      >
        halt
      </button>
    </div>
  );
}
