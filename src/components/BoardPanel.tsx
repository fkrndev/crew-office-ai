import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface Task {
  id?: string;
  title?: string;
  status?: string;
  owner?: string;
  notes?: string;
}
const hiveBoard = () => invoke<string>('hive_board');
const hiveTasks = () => invoke<{ tasks: Task[] }>('hive_tasks');

/** The shared board + task ledger god maintains — board.md and tasks.json
 *  are FILES the god agent edits itself; this panel only ever reads them. */
export function BoardPanel({ onClose }: { onClose: () => void }) {
  const [board, setBoard] = useState('');
  const [tasks, setTasks] = useState<Task[]>([]);

  useEffect(() => {
    const refresh = () => {
      hiveBoard().then(setBoard);
      hiveTasks().then((t) => setTasks(t.tasks ?? []));
    };
    refresh();
    const iv = setInterval(refresh, 5000);
    const un = listen('hive-deliver', refresh);
    return () => {
      clearInterval(iv);
      un.then((f) => f());
    };
  }, []);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <h2>Board</h2>
        <div className="split">
          <div className="board-md">
            <pre>{board || '(empty — god has not written the board yet)'}</pre>
          </div>
          <div className="board-tasks">
            <h3>Tasks ({tasks.length})</h3>
            {tasks.map((t, i) => (
              <div key={t.id ?? i} className="mission-row">
                <div className="grow">
                  <div className="agent-name">{t.title ?? '(untitled)'}</div>
                  <div className="agent-sub">
                    {t.owner ?? 'unowned'} {t.notes ? `· ${t.notes}` : ''}
                  </div>
                </div>
                <span className="tag">{t.status ?? '?'}</span>
              </div>
            ))}
            {tasks.length === 0 && <p className="empty">No tasks on the ledger.</p>}
          </div>
        </div>
        <div className="row end">
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
