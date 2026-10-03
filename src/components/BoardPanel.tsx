import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface Ask {
  q: string;
  askedAt?: string;
  a?: string;
  answeredAt?: string;
}
interface Task {
  id?: string;
  title?: string;
  status?: string;
  assignee?: string;
  owner?: string;
  notes?: string;
  humanQA?: Ask[];
}
const hiveBoard = () => invoke<string>('hive_board');
const hiveTasks = () => invoke<{ tasks: Task[] }>('hive_tasks');
const taskUpdate = (id: string, patch: Record<string, unknown>) =>
  invoke('task_update', { id, patch });
const taskAnswer = (id: string, answer: string) => invoke('task_answer', { id, answer });

const STATUSES = ['backlog', 'in-progress', 'review', 'blocked', 'done'];

/** The shared board + task ledger god maintains. board.md stays read-only
 *  (god is its sole scribe); the task ledger accepts the two human verbs the
 *  upstream ASK ME board and kanban support — status moves and humanQA
 *  answers (patchTaskInLedger semantics, unknown fields preserved). */
export function BoardPanel({ onClose }: { onClose: () => void }) {
  const [board, setBoard] = useState('');
  const [tasks, setTasks] = useState<Task[]>([]);
  const [answers, setAnswers] = useState<Record<string, string>>({});

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

  const openAsk = (t: Task) => (t.humanQA || []).find((q) => !q.a);

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
              <div key={t.id ?? i} className="mission-row" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
                <div className="row">
                  <div className="grow">
                    <div className="agent-name">{t.title ?? '(untitled)'}</div>
                    <div className="agent-sub">
                      {t.assignee ?? t.owner ?? 'unowned'} {t.notes ? `· ${t.notes}` : ''}
                    </div>
                  </div>
                  {t.id && (
                    <select
                      value={t.status ?? 'backlog'}
                      onChange={(e) => taskUpdate(t.id!, { status: e.target.value })}
                    >
                      {STATUSES.map((s) => (
                        <option key={s} value={s}>
                          {s}
                        </option>
                      ))}
                    </select>
                  )}
                  {!t.id && <span className="tag">{t.status ?? '?'}</span>}
                </div>
                {openAsk(t) && (
                  <div className="ask">
                    <div className="agent-sub" style={{ whiteSpace: 'pre-wrap' }}>
                      <b>ASK:</b> {openAsk(t)!.q}
                    </div>
                    <div className="row">
                      <input
                        placeholder="answer…"
                        value={answers[t.id ?? ''] ?? ''}
                        onChange={(e) =>
                          setAnswers({ ...answers, [t.id ?? '']: e.target.value })
                        }
                        style={{ flex: 1 }}
                      />
                      <button
                        onClick={() => {
                          const a = answers[t.id ?? '']?.trim();
                          if (t.id && a) {
                            taskAnswer(t.id, a);
                            setAnswers({ ...answers, [t.id]: '' });
                          }
                        }}
                      >
                        Send
                      </button>
                    </div>
                  </div>
                )}
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
