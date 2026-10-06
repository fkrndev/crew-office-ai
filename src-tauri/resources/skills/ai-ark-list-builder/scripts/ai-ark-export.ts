#!/usr/bin/env tsx
// Build a lead list from AI Ark: preview cheaply, export async, poll, download CSV.
//
// Usage:
//   npx tsx scripts/ai-ark-export.ts --preview --config targeting.json
//       Search preview: prints total matches + first sample rows. Costs little to nothing.
//   npx tsx scripts/ai-ark-export.ts --submit --config targeting.json
//       Starts the paid export (about 1 credit per record). Confirms before spending.
//       Persists trackIds to .aiark-tracks.json so a killed run is resumable.
//   npx tsx scripts/ai-ark-export.ts --fetch
//       Polls the saved trackIds to DONE, downloads everything, writes export-leads.csv.
//
// targeting.json shape (all fields optional except size for submit):
// {
//   "titlesInclude": ["Head of Marketing", "VP Marketing"],
//   "titlesExclude": ["Assistant"],
//   "industriesInclude": [],
//   "industriesExclude": ["Staffing and Recruiting"],
//   "employeeSize": [50, 1000],
//   "locations": ["North America::United States"],
//   "companyKeywordInclude": [],
//   "companyKeywordExclude": ["agency"],
//   "size": 2000
// }

import fs from "node:fs";
import { env, required, parseArgs, writeCsv, sleep, confirm } from "./_lib.ts";

const BASE = "https://api.ai-ark.com/api/developer-portal/v1";
const TRACK_FILE = ".aiark-tracks.json";
// AI Ark requires a webhook URL on exports even if you never receive it.
// We poll instead, so any syntactically valid URL works here.
const WEBHOOK_SINK = "https://example.com/webhook-sink";

const KEY = required("AI_ARK_API_KEY");
const H = { "X-TOKEN": KEY, "Content-Type": "application/json" };

async function api(path: string, init: RequestInit = {}): Promise<any> {
  for (let a = 0; a < 30; a++) {
    let r: Response;
    try {
      r = await fetch(`${BASE}${path}`, { ...init, headers: { ...H, ...(init.headers ?? {}) } });
    } catch {
      await sleep(5000);
      continue;
    }
    // 429 = rate limited, 409 = export still processing. Both mean wait and retry.
    if (r.status === 429 || r.status === 409) {
      await sleep(Math.min(20000, 5000 * (a + 1)));
      continue;
    }
    const t = await r.text();
    let j: any;
    try { j = JSON.parse(t); } catch { j = { raw: t.slice(0, 200) }; }
    if (!r.ok) throw new Error(`${r.status} ${path}: ${JSON.stringify(j).slice(0, 300)}`);
    return j;
  }
  throw new Error(`retry-exhausted: ${path}`);
}

// SearchMatch shape: {"any":{"include":{"mode","content"},"exclude":{...}}}
function searchMatch(include: string[] = [], exclude: string[] = [], mode = "WORD"): any | undefined {
  if (include.length === 0 && exclude.length === 0) return undefined;
  const any: any = {};
  if (include.length) any.include = { mode, content: include };
  if (exclude.length) any.exclude = { mode, content: exclude };
  return { any };
}

// Company keyword needs the special "sources" shape or the API rejects it (401).
function keywordFilter(include: string[] = [], exclude: string[] = []): any | undefined {
  if (include.length === 0 && exclude.length === 0) return undefined;
  const sources = ["NAME", "KEYWORD", "SEO", "DESCRIPTION"].map(source => ({ mode: "WORD", source }));
  const any: any = {};
  if (include.length) any.include = { sources, content: include };
  if (exclude.length) any.exclude = { sources, content: exclude };
  return { any };
}

function buildBody(cfg: any, size: number, forExport: boolean, page = 0): any {
  const account: any = {};
  if (cfg.employeeSize?.length === 2) {
    account.employeeSize = { type: "RANGE", range: [{ start: cfg.employeeSize[0], end: cfg.employeeSize[1] }] };
  }
  const industries = searchMatch(cfg.industriesInclude, cfg.industriesExclude);
  if (industries) account.industries = industries;
  if (cfg.locations?.length) account.location = { any: { include: cfg.locations } };
  // Keyword logic belongs at the ACCOUNT level. Person-level keywords are unreliable.
  const kw = keywordFilter(cfg.companyKeywordInclude, cfg.companyKeywordExclude);
  if (kw) account.keyword = kw;

  const contact: any = {};
  const title = searchMatch(cfg.titlesInclude, cfg.titlesExclude);
  if (title) contact.experience = { current: { title } };

  const body: any = { page, size };
  if (Object.keys(account).length) body.account = account;
  if (Object.keys(contact).length) body.contact = contact;
  if (forExport) body.webhook = cfg.webhook ?? WEBHOOK_SINK;
  return body;
}

function loadConfig(flags: Record<string, string | boolean>): any {
  const p = (flags.config as string) || "targeting.json";
  if (!fs.existsSync(p)) {
    console.error(`No targeting config at ${p}. Create one (see the header of this script for the shape).`);
    process.exit(1);
  }
  return JSON.parse(fs.readFileSync(p, "utf8"));
}

function rowFromResult(it: any): Record<string, string> {
  const p = it.profile ?? {};
  return {
    email: String(it.email?.output?.[0]?.address ?? "").toLowerCase(),
    first_name: p.first_name ?? "",
    last_name: p.last_name ?? "",
    company: it.company?.summary?.name ?? "",
    title: it.title ?? p.title ?? "",
    linkedin: it.link?.linkedin ?? "",
    industry: it.company?.summary?.industry ?? "",
    website: it.company?.summary?.website ?? "",
  };
}

function printSample(rows: any[]) {
  for (const it of rows) {
    const r = rowFromResult(it);
    console.log(`  ${(r.first_name + " " + r.last_name).trim().padEnd(24)} ${r.title.padEnd(36).slice(0, 36)} ${r.company}`);
  }
}

async function preview(cfg: any) {
  const PAGE_SIZE = 10;
  console.log("Search preview (not the paid export)...\n");
  const front = await api("/people", { method: "POST", body: JSON.stringify(buildBody(cfg, PAGE_SIZE, false, 0)) });
  const total = Number(front.totalElements ?? front.total ?? 0);
  const frontRows = front.content ?? [];
  console.log(`Total matches: ${total || "?"}`);
  console.log(`\nFront page (best matches first):\n`);
  printSample(frontRows);

  // AI Ark results are NOT randomly ordered; the worst matches cluster at the end.
  // Page 1 always looks clean, so the back page is the one that tells you the truth.
  // The API caps how deep you can page (a 10,000-record result window), so for a
  // very broad search the true last page isn't reachable. Go as deep as allowed instead.
  if (total > PAGE_SIZE) {
    const MAX_OFFSET = 9999;
    const trueLastPage = Math.floor((total - 1) / PAGE_SIZE);
    const deepestReachable = Math.floor(MAX_OFFSET / PAGE_SIZE);
    const targetPage = Math.min(trueLastPage, deepestReachable);
    const back = await api("/people", { method: "POST", body: JSON.stringify(buildBody(cfg, PAGE_SIZE, false, targetPage)) });
    if (targetPage < trueLastPage) {
      console.log(`\nDeepest page the API allows (page ${targetPage + 1}; the true last page would be ${trueLastPage + 1}):`);
      console.log(`${total} matches is too broad to see the true tail. If this page still looks clean, narrow anyway: a search this wide is diluting, not widening. See "generic single words" above.\n`);
    } else {
      console.log(`\nBack page (page ${targetPage + 1} of ${targetPage + 1}, the honest one):\n`);
    }
    printSample(back.content ?? []);
  }
  console.log(`\nIf the sample looks right, export with:`);
  console.log(`  npx tsx scripts/ai-ark-export.ts --submit --config <your config>`);
  console.log(`Export cost: about 1 credit per record. Set "size" in the config to control spend.`);
}

async function submit(cfg: any, skipConfirm: boolean) {
  const size = Number(cfg.size ?? 0);
  if (!size || size < 1 || size > 10000) {
    console.error(`Config "size" must be 1-10000 (got ${cfg.size}). This controls how many records you pay for.`);
    process.exit(1);
  }
  const credits = await api("/payments/credits").catch(() => null);
  const balance = credits?.total ?? "unknown";
  console.log(`Export size: ${size} records (about ${size} credits). Credit balance: ${balance}.`);
  if (!skipConfirm) {
    const ok = await confirm(`Start the paid export?`);
    if (!ok) { console.log("Cancelled. Nothing spent."); process.exit(0); }
  }
  const body = buildBody(cfg, size, true);
  const job = await api("/people/export", { method: "POST", body: JSON.stringify(body) });
  const trackId = job.trackId ?? job.track_id ?? job.id;
  if (!trackId) throw new Error(`No trackId in response: ${JSON.stringify(job).slice(0, 300)}`);
  const tracks = fs.existsSync(TRACK_FILE) ? JSON.parse(fs.readFileSync(TRACK_FILE, "utf8")) : [];
  tracks.push({ trackId, size, submittedAt: new Date().toISOString() });
  fs.writeFileSync(TRACK_FILE, JSON.stringify(tracks, null, 2));
  console.log(`Submitted. trackId ${trackId} saved to ${TRACK_FILE}.`);
  console.log(`Results take minutes to hours depending on size. Download with:`);
  console.log(`  npx tsx scripts/ai-ark-export.ts --fetch`);
}

async function pollStatistics(trackId: string): Promise<any> {
  // The proven endpoint is /statistics; some docs show /inquiries/statistics. Try both.
  try {
    return await api(`/people/export/${trackId}/statistics`);
  } catch (e: any) {
    if (String(e.message).startsWith("404")) return await api(`/people/export/${trackId}/inquiries/statistics`);
    throw e;
  }
}

async function fetchAll() {
  if (!fs.existsSync(TRACK_FILE)) {
    console.error(`No ${TRACK_FILE} found. Run --submit first.`);
    process.exit(1);
  }
  const tracks = JSON.parse(fs.readFileSync(TRACK_FILE, "utf8"));
  const all: Record<string, string>[] = [];
  for (const t of tracks) {
    for (let i = 0; i < 240; i++) {
      const st = await pollStatistics(t.trackId);
      const state = String(st.state ?? "").toUpperCase();
      const found = st.statistics?.found ?? 0;
      const total = st.statistics?.total ?? "?";
      process.stderr.write(`\r[${t.trackId.slice(0, 8)}] ${state} found=${found}/${total}   `);
      if (state === "DONE" || state === "COMPLETED") break;
      if (state === "FAILED") throw new Error(`Export ${t.trackId} FAILED`);
      await sleep(30000);
    }
    process.stderr.write("\n");
    let page = 0;
    while (true) {
      const res = await api(`/people/export/${t.trackId}/inquiries?page=${page}&size=100`);
      const content = res.content ?? [];
      for (const it of content) {
        const row = rowFromResult(it);
        if (row.email) all.push(row);
      }
      if (res.last || content.length === 0) break;
      page++;
    }
  }
  // dedupe by email
  const seen = new Set<string>();
  const uniq = all.filter(r => !seen.has(r.email) && seen.add(r.email));
  writeCsv("export-leads.csv", uniq);
  console.log(`\nDone: ${uniq.length} unique leads with emails written to export-leads.csv`);
  console.log(`Quality check tip: inspect the LAST rows of the file. Worst matches cluster at the back.`);
  console.log(`Next: /list-hygiene (dedup + MX exclusions), then /emailbison-campaign-launch.`);
}

async function main() {
  const { flags } = parseArgs();
  if (flags.preview) return preview(loadConfig(flags));
  if (flags.submit) return submit(loadConfig(flags), !!flags.yes);
  if (flags.fetch) return fetchAll();
  console.log("Usage: --preview | --submit | --fetch  [--config targeting.json] [--yes]");
}

main().catch(e => { console.error(`\n${e.message}`); process.exit(1); });
