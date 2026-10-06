#!/usr/bin/env tsx
// Pull campaign performance from EmailBison. Read-only, spends nothing.
//
// Usage:
//   npx tsx scripts/emailbison-pull-analytics.ts                     # all campaigns, one table
//   npx tsx scripts/emailbison-pull-analytics.ts --campaign-id 123 --days 14   # daily volume for one campaign

import { required, parseArgs, sleep, printTable } from "./_lib.ts";

const BASE = required("EMAILBISON_BASE_URL").replace(/\/+$/, "");
const KEY = required("EMAILBISON_API_KEY");

async function api(path: string, method = "GET", body?: any): Promise<any> {
  for (let a = 0; a < 4; a++) {
    const r = await fetch(`${BASE}${path}`, {
      method,
      headers: { Authorization: `Bearer ${KEY}`, Accept: "application/json", "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (r.status === 429 || r.status >= 500) { await sleep(1000 * Math.pow(2, a)); continue; }
    const text = await r.text();
    let j: any; try { j = JSON.parse(text); } catch { j = {}; }
    if (!r.ok) throw new Error(`${r.status} ${method} ${path}: ${text.slice(0, 300)}`);
    if (j?.data && !Array.isArray(j.data) && typeof j.data === "object" && j.data.success === false) {
      throw new Error(`API rejected the call (auth or workspace mismatch): ${j.data.message ?? "unknown"}`);
    }
    return j;
  }
  throw new Error(`retry-exhausted: ${method} ${path}`);
}

// EmailBison paginates at 15/page no matter what you ask for. Loop to meta.last_page.
async function paginate(path: string, maxPages = 200): Promise<any[]> {
  const all: any[] = [];
  let page = 1;
  let lastPage = 1;
  do {
    const res = await api(`${path}${path.includes("?") ? "&" : "?"}page=${page}`);
    const data = Array.isArray(res?.data) ? res.data : [];
    all.push(...data);
    lastPage = res?.meta?.last_page ?? 1;
    page++;
  } while (page <= lastPage && page <= maxPages);
  return all;
}

function pct(n: number, d: number): string {
  return d > 0 ? `${((n / d) * 100).toFixed(1)}%` : "-";
}

async function main() {
  const { flags } = parseArgs();

  if (flags["campaign-id"]) {
    const id = flags["campaign-id"];
    const days = Number(flags.days ?? 14);
    const end = new Date();
    const start = new Date(end.getTime() - days * 86400000);
    const fmt = (d: Date) => d.toISOString().slice(0, 10);
    // line-area-chart-stats is the ONLY true daily-volume read. Both dates are required.
    const res = await api(`/api/campaigns/${id}/line-area-chart-stats?start_date=${fmt(start)}&end_date=${fmt(end)}`);
    console.log(JSON.stringify(res, null, 2));
    return;
  }

  console.log("Pulling all campaigns (15/page, so this takes a moment)...\n");
  const campaigns = await paginate("/api/campaigns");
  const rows = campaigns.map((c: any) => {
    const sent = Number(c.emails_sent ?? 0);
    const replies = Number(c.unique_replies ?? c.replied ?? 0);
    const interested = Number(c.interested ?? 0);
    return {
      campaign: String(c.name ?? "").slice(0, 40),
      status: String(c.status ?? ""),
      leads: String(c.total_leads ?? ""),
      sent: String(sent),
      "bounce%": pct(Number(c.bounced ?? 0), sent),
      replies: String(replies),
      "interested%ofReplies": pct(interested, replies),
    };
  });
  printTable(rows);
  console.log(`\n${campaigns.length} campaigns.`);
  console.log(`The number that matters: interested as a share of replies. 20%+ great, 10%+ good, under 5% means offer or list.`);
}

main().catch(e => { console.error(`\n${e.message}`); process.exit(1); });
