import i18n from 'i18next';

type SpawnArgs = Record<string, unknown> & { id: string };
// The bridge is `[key: string]: any` by design — mirroring it keeps every
// existing spawnPty call site's field access compiling verbatim.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type SpawnRes = any;

/**
 * spawnPty with a built-in ShardX rescue. When the spawn comes back
 * needsShardx — the agent was armed for it (consent granted) but the
 * vendored bridge or stored token is missing — the human gets ONE
 * confirm; accepting runs the zero-touch bootstrap and the spawn is
 * retried so the process actually lands its MCP tools. Declining or a
 * failed bootstrap returns the original (armed-but-toolless) spawn.
 */
export async function spawnWithShardxSetup(args: SpawnArgs): Promise<SpawnRes> {
  const res = await window.cth.spawnPty(args);
  if (!res.ok || !res.needsShardx) return res;
  if (!window.confirm(i18n.t('shardx.autoSetupConfirm'))) return res;
  const boot = await window.cth.shardxBootstrap().catch(() => null);
  if (!boot?.ok) return res;
  await window.cth.killPty(args.id).catch(() => undefined);
  return window.cth.spawnPty(args);
}
