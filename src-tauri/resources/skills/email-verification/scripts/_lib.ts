// Shared utilities for all cold-email-playbooks scripts.
// Import with: import { env, required, readCsv, writeCsv, sleep, retry, createQueue, confirm, printTable, parseArgs } from "./_lib.ts";
//
// Adapted from GrowthEngineX's coldoutboundskills _lib.ts (MIT).
// Zero dependencies: everything here is Node stdlib.

import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

// ─────────────────────────────────────────────────────────
// .env loading
// Walks UP from this file's directory until it finds a .env,
// so scripts work both inside the full repo (root .env) and
// when a skill folder is copied out standalone (local .env).
// Real environment variables always win over .env values.
// ─────────────────────────────────────────────────────────
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export function loadEnv(): Record<string, string> {
  const out: Record<string, string> = { ...(process.env as Record<string, string>) };
  // Max 5 levels: scripts/ → skill folder → .claude/skills/ → .claude/ → repo root.
  // Never climbs above the repo, so a stray .env elsewhere on disk is never picked up.
  let dir = __dirname;
  for (let depth = 0; depth < 5; depth++) {
    const envPath = path.join(dir, ".env");
    if (fs.existsSync(envPath)) {
      for (const line of fs.readFileSync(envPath, "utf8").split("\n")) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith("#")) continue;
        const eq = trimmed.indexOf("=");
        if (eq === -1) continue;
        const key = trimmed.slice(0, eq).trim();
        let val = trimmed.slice(eq + 1).trim();
        if ((val.startsWith('"') && val.endsWith('"')) || (val.startsWith("'") && val.endsWith("'"))) {
          val = val.slice(1, -1);
        }
        if (out[key] === undefined || out[key] === "") out[key] = val;
      }
      return out;
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return out;
}
export const env = loadEnv();

export function required(key: string): string {
  const v = env[key];
  if (!v) throw new Error(`Missing required env var: ${key}. Copy .env.example to .env and fill it in.`);
  return v;
}

// ─────────────────────────────────────────────────────────
// CLI args (tiny parser, no dependency). Repeated flags: last one wins.
// ─────────────────────────────────────────────────────────
export function parseArgs(argv: string[] = process.argv.slice(2)): { flags: Record<string, string | boolean>; positional: string[] } {
  const flags: Record<string, string | boolean> = {};
  const positional: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next !== undefined && !next.startsWith("--")) {
        flags[key] = next;
        i++;
      } else {
        flags[key] = true;
      }
    } else {
      positional.push(a);
    }
  }
  return { flags, positional };
}

// ─────────────────────────────────────────────────────────
// CSV (minimal, handles quoted fields and embedded newlines in quotes)
// ─────────────────────────────────────────────────────────
export function readCsv(filepath: string): Record<string, string>[] {
  const raw = fs.readFileSync(filepath, "utf8");
  const records = parseCsv(raw);
  if (records.length === 0) return [];
  const headers = records[0];
  return records.slice(1).map(values => {
    const row: Record<string, string> = {};
    headers.forEach((h, i) => { row[h] = values[i] ?? ""; });
    return row;
  });
}

export function writeCsv(filepath: string, rows: Record<string, any>[]): void {
  if (rows.length === 0) {
    fs.writeFileSync(filepath, "");
    return;
  }
  const headers = Array.from(rows.reduce<Set<string>>((s, r) => { Object.keys(r).forEach(k => s.add(k)); return s; }, new Set()));
  const out: string[] = [headers.map(escapeCsv).join(",")];
  for (const r of rows) {
    out.push(headers.map(h => escapeCsv(r[h])).join(","));
  }
  fs.writeFileSync(filepath, out.join("\n") + "\n");
}

function parseCsv(raw: string): string[][] {
  const records: string[][] = [];
  let record: string[] = [];
  let cur = "";
  let inQuote = false;
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (inQuote) {
      if (c === '"' && raw[i + 1] === '"') { cur += '"'; i++; }
      else if (c === '"') { inQuote = false; }
      else { cur += c; }
    } else if (c === '"') {
      inQuote = true;
    } else if (c === ",") {
      record.push(cur); cur = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && raw[i + 1] === "\n") i++;
      record.push(cur); cur = "";
      if (record.length > 1 || record[0] !== "") records.push(record);
      record = [];
    } else {
      cur += c;
    }
  }
  if (cur !== "" || record.length > 0) {
    record.push(cur);
    if (record.length > 1 || record[0] !== "") records.push(record);
  }
  return records;
}

function escapeCsv(v: any): string {
  if (v == null) return "";
  const s = String(v);
  if (s.includes(",") || s.includes('"') || s.includes("\n")) {
    return `"${s.replace(/"/g, '""')}"`;
  }
  return s;
}

// ─────────────────────────────────────────────────────────
// Async utilities
// ─────────────────────────────────────────────────────────
export const sleep = (ms: number) => new Promise<void>(r => setTimeout(r, ms));

export async function retry<T>(fn: () => Promise<T>, opts: { attempts?: number; baseDelayMs?: number; onAttemptError?: (e: any, attempt: number) => void } = {}): Promise<T> {
  const { attempts = 5, baseDelayMs = 1000, onAttemptError } = opts;
  let lastErr: any;
  for (let i = 0; i < attempts; i++) {
    try { return await fn(); } catch (e: any) {
      lastErr = e;
      onAttemptError?.(e, i + 1);
      if (i < attempts - 1) await sleep(Math.min(baseDelayMs * Math.pow(2, i), 30000));
    }
  }
  throw lastErr;
}

// simple concurrency limiter (p-queue replacement)
export function createQueue(concurrency: number) {
  let running = 0;
  const queue: Array<() => void> = [];
  const next = () => {
    if (running >= concurrency) return;
    const task = queue.shift();
    if (!task) return;
    running++;
    task();
  };
  return {
    add<T>(fn: () => Promise<T>): Promise<T> {
      return new Promise((resolve, reject) => {
        queue.push(() => {
          fn().then(resolve, reject).finally(() => { running--; next(); });
        });
        next();
      });
    },
  };
}

// ─────────────────────────────────────────────────────────
// HTTP helper: fetch + JSON with useful errors
// ─────────────────────────────────────────────────────────
export async function fetchJson<T = any>(url: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(url, init);
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`HTTP ${res.status} ${res.statusText} for ${init.method ?? "GET"} ${url}\n${text.slice(0, 500)}`);
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new Error(`Non-JSON response from ${url}: ${text.slice(0, 200)}`);
  }
}

// ─────────────────────────────────────────────────────────
// Terminal
// ─────────────────────────────────────────────────────────
export async function confirm(question: string): Promise<boolean> {
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  return new Promise(resolve => {
    rl.question(`${question} [y/N] `, answer => {
      rl.close();
      resolve(answer.toLowerCase().startsWith("y"));
    });
  });
}

export function printTable(rows: Array<Record<string, string>>): void {
  if (rows.length === 0) return;
  const headers = Object.keys(rows[0]);
  const widths = headers.map(h => Math.max(h.length, ...rows.map(r => String(r[h] ?? "").length)));
  const sep = "  ";
  console.log(headers.map((h, i) => h.padEnd(widths[i])).join(sep));
  console.log(widths.map(w => "─".repeat(w)).join(sep));
  for (const r of rows) {
    console.log(headers.map((h, i) => String(r[h] ?? "").padEnd(widths[i])).join(sep));
  }
}
