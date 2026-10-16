#!/usr/bin/env node
/**
 * Fast local build — for trying a change, NOT for shipping.
 *
 * The release profile in src-tauri/Cargo.toml uses fat LTO with one codegen
 * unit: the smallest, fastest binary, but the compile is effectively
 * single-threaded (a rebuild of our own crate alone measured ~16 min). This
 * script builds the same release profile with thin LTO and 16 codegen units
 * through Cargo's CARGO_PROFILE_RELEASE_* overrides, so Cargo.toml (and the
 * official release build / CI) stays untouched.
 *
 *   npm run tauri:build:fast              # exe only (--no-bundle), fastest
 *   npm run tauri:build:fast -- --bundle  # also the NSIS installer
 *
 * It uses its own target dir (src-tauri/target-fast) so switching between
 * this and a normal `npm run tauri:build` never invalidates the other's
 * cache, and it turns the signed-updater artifacts off so no signing key is
 * needed. Output: src-tauri/target-fast/release/agent-office.exe
 */
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const wantBundle = process.argv.includes('--bundle');

const dir = mkdtempSync(join(tmpdir(), 'agent-office-fast-'));
const cfg = join(dir, 'no-updater.json');
writeFileSync(cfg, JSON.stringify({ bundle: { createUpdaterArtifacts: false } }));

const env = {
  ...process.env,
  CARGO_TARGET_DIR: join(root, 'src-tauri', 'target-fast'),
  CARGO_PROFILE_RELEASE_LTO: 'thin',
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS: '16',
};

const args = ['tauri', 'build', '--config', cfg];
if (wantBundle) args.push('--bundles', 'nsis');
else args.push('--no-bundle');

console.log(`[build-fast] thin LTO, 16 codegen units, target dir ${env.CARGO_TARGET_DIR}`);
const started = Date.now();
const r = spawnSync('npx', args, { cwd: root, env, stdio: 'inherit', shell: process.platform === 'win32' });
const secs = Math.round((Date.now() - started) / 1000);
console.log(`[build-fast] finished in ${Math.floor(secs / 60)}m ${secs % 60}s (exit ${r.status})`);
process.exit(r.status ?? 1);
