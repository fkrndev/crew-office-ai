import { useEffect, useMemo, useState, useCallback } from 'react';
import CodeMirror from '@uiw/react-codemirror';
import { EditorView } from '@codemirror/view';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { markdown } from '@codemirror/lang-markdown';
import { python } from '@codemirror/lang-python';
import { html } from '@codemirror/lang-html';
import { css } from '@codemirror/lang-css';
import { yaml } from '@codemirror/lang-yaml';
import { useTranslation } from 'react-i18next';
import { Icon } from './Icon';
import { PixelButton } from './PixelButton';

// ─── Theme matching CTH palette ─────────────────────────────────────────────
const cthEditorTheme = EditorView.theme({
  '&': {
    background: '#FBFCFD',
    color: '#0F1C2E',
    height: '100%',
    fontFamily: 'VT323, "JetBrains Mono", monospace',
    fontSize: '16px'
  },
  '.cm-content': { caretColor: '#1B9890', padding: '8px 0' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: '#1B9890', borderLeftWidth: '2px' },
  '.cm-scroller': { fontFamily: 'inherit', overflow: 'auto' },
  '.cm-gutters': {
    background: '#EDF1F5',
    color: '#50627A',
    borderRight: '1px solid #CCD6E1'
  },
  '.cm-activeLineGutter': { background: '#BEE3EE' },
  '.cm-activeLine': { background: 'rgba(27, 152, 144, 0.08)' },
  '.cm-selectionBackground, ::selection': { background: '#BEE3EE !important' },
  '.cm-searchMatch': { background: '#BDE3F5', outline: '1px solid #0F1C2E' },
  '.cm-searchMatch.cm-searchMatch-selected': { background: '#F2D06B' }
}, { dark: false });

const cthSyntax = HighlightStyle.define([
  { tag: tags.keyword,        color: '#5B50C8' },
  { tag: tags.operator,       color: '#50627A' },
  { tag: [tags.string, tags.regexp], color: '#1B7F57' },
  { tag: [tags.number, tags.bool, tags.null], color: '#C23B48' },
  { tag: tags.comment,        color: '#50627A', fontStyle: 'italic' },
  { tag: tags.variableName,   color: '#0F1C2E' },
  { tag: tags.function(tags.variableName), color: '#B5532B' },
  { tag: [tags.typeName, tags.className], color: '#12797A' },
  { tag: tags.propertyName,   color: '#2C3E55' },
  { tag: tags.heading,        color: '#0F1C2E', fontWeight: 'bold' as any },
  { tag: tags.link,           color: '#176D6E', textDecoration: 'underline' as any },
  { tag: tags.meta,           color: '#50627A' }
]);

function extensionsFor(filename: string) {
  const ext = filename.split('.').pop()?.toLowerCase() ?? '';
  if (['ts', 'tsx'].includes(ext)) return [javascript({ jsx: true, typescript: true })];
  if (['js', 'jsx', 'mjs', 'cjs'].includes(ext)) return [javascript({ jsx: ext.endsWith('x') })];
  if (ext === 'json') return [json()];
  if (['md', 'markdown'].includes(ext)) return [markdown()];
  if (ext === 'py') return [python()];
  if (['html', 'htm'].includes(ext)) return [html()];
  if (ext === 'css') return [css()];
  if (['yml', 'yaml'].includes(ext)) return [yaml()];
  return [];
}

export interface CodeEditorProps {
  root: string;
  /** Relative file path within `root` */
  filePath: string | null;
  /** Escalate this file into the IDE. The sidebar editor is deliberately
   *  small; the IDE is where tabs, the tree, git, and markdown preview live. */
  onOpenInIde?: () => void;
  onCopyPath?: () => void;
}

export function CodeEditor({
  root, filePath, onOpenInIde, onCopyPath
}: CodeEditorProps) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string>('');
  const [originalContent, setOriginalContent] = useState<string>('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>();
  const [absPath, setAbsPath] = useState<string | undefined>();
  const [saveState, setSaveState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');

  // Load file on path change
  useEffect(() => {
    let cancelled = false;
    if (!filePath) {
      setContent(''); setOriginalContent(''); setError(undefined);
      setAbsPath(undefined);
      return;
    }
    setLoading(true);
    setError(undefined);
    window.cth.readFile(root, filePath).then(res => {
      if (cancelled) return;
      setLoading(false);
      if (res.ok) {
        setContent(res.content);
        setOriginalContent(res.content);
        setAbsPath(res.path);
      } else {
        setContent('');
        setOriginalContent('');
        setAbsPath(undefined);
        setError(res.error);
      }
    });
    return () => { cancelled = true; };
  }, [root, filePath]);

  const dirty = content !== originalContent;

  const save = useCallback(async () => {
    if (!filePath || !dirty) return;
    setSaveState('saving');
    let res: { ok: boolean; error?: string };
    try {
      res = await window.cth.writeFile(root, filePath, content);
    } catch (e) {
      // A rejected write must not leave the button on "saving…" forever.
      res = { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
    if (res.ok) {
      setOriginalContent(content);
      setSaveState('saved');
      setTimeout(() => setSaveState('idle'), 1200);
    } else {
      setSaveState('error');
      setError(res.error);
      setTimeout(() => setSaveState('idle'), 4000);
    }
  }, [filePath, dirty, content, root]);

  // Cmd-S to save
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [save]);

  const extensions = useMemo(
    () => [cthEditorTheme, syntaxHighlighting(cthSyntax), ...(filePath ? extensionsFor(filePath) : [])],
    [filePath]
  );

  if (!filePath) {
    return (
      <div style={{
        height: '100%', display: 'flex', flexDirection: 'column',
        alignItems: 'center', justifyContent: 'center', gap: 12,
        background: 'var(--cth-paper-200)',
        textAlign: 'center'
      }}>
        <div style={{ opacity: 0.5 }}>
          <Icon name="code" size={2} />
        </div>
        <div style={{
          fontFamily: 'var(--cth-font-display)', fontSize: 8, lineHeight: '14px',
          textTransform: 'uppercase', letterSpacing: 1,
          color: 'var(--cth-ink-700)'
        }}>
          No file open
        </div>
        <div style={{
          fontFamily: 'var(--cth-font-ui)', fontSize: 13,
          color: 'var(--cth-ink-500)'
        }}>
          Pick a file from the tree to view it here.
        </div>
      </div>
    );
  }

  return (
    <div style={{
      height: '100%',
      display: 'flex', flexDirection: 'column',
      background: 'var(--cth-paper-100)'
    }}>
      {/* Mini header */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 6,
        padding: '4px 8px',
        background: 'var(--cth-cream-200)',
        borderBottom: '1px solid var(--cth-ink-700)',
        fontFamily: 'var(--cth-font-ui)', fontSize: 12,
        color: 'var(--cth-ink-700)'
      }}>
        <Icon name="code" />
        <span style={{
          flex: 1, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'
        }} title={absPath}>{filePath}{dirty && ' •'}</span>
        {onCopyPath && (
          <button
            onClick={onCopyPath}
            title={t('codeEditor.copyPathTitle')}
            style={editorBtn}
          >{t('codeEditor.copyPath')}</button>
        )}
        <button
          onClick={save}
          disabled={!dirty || saveState === 'saving'}
          title={window.cth?.platform === 'darwin' ? t('codeEditor.saveTitleMac') : t('codeEditor.saveTitleOther')}
          style={{ ...editorBtn, opacity: dirty ? 1 : 0.5 }}
        >
          {saveState === 'saving' ? '...' : saveState === 'saved' ? t('codeEditor.saved') : saveState === 'error' ? t('codeEditor.error') : t('codeEditor.save')}
        </button>
        {onOpenInIde && (
          <button
            onClick={onOpenInIde}
            title={t('codeEditor.openInIde')}
            aria-label={t('codeEditor.openInIde')}
            style={editorBtn}
          >
            <Icon name="code" />
          </button>
        )}
      </div>

      {/* Body */}
      <div style={{ flex: 1, minHeight: 0, overflow: 'hidden' }}>
        {loading ? (
          <div style={{ padding: 12, color: 'var(--cth-ink-500)' }}>{t('codeEditor.loading')}</div>
        ) : error ? (
          <div style={{ padding: 12, color: 'var(--cth-coral)' }}>{error}</div>
        ) : (
          <CodeMirror
            value={content}
            onChange={(v) => setContent(v)}
            extensions={extensions}
            height="100%"
            theme="light"
            basicSetup={{
              lineNumbers: true,
              highlightActiveLine: true,
              foldGutter: true,
              autocompletion: false
            }}
          />
        )}
      </div>
    </div>
  );
}

const editorBtn: React.CSSProperties = {
  padding: '0 6px', height: 22,
  fontFamily: 'var(--cth-font-ui)', fontSize: 12,
  color: 'var(--cth-ink-900)',
  background: 'var(--cth-cream-100)',
  border: 'none',
  boxShadow: 'inset 0 0 0 1px var(--cth-ink-100)',
  cursor: 'pointer',
  display: 'inline-flex', alignItems: 'center', gap: 4
};
