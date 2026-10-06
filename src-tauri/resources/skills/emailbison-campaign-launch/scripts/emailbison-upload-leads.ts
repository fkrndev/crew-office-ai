#!/usr/bin/env tsx
// Upload a leads CSV into EmailBison and attach the leads to a campaign.
//
// Usage:
//   npx tsx scripts/emailbison-upload-leads.ts --campaign-id 123 --csv sendable-leads.csv
//
// CSV columns used: email (required), first_name, last_name, company, title, phone, website.
// Any OTHER column becomes a custom variable ({{COLUMN_NAME}} in your copy, uppercased).
// Never name a column "company" AND expect a custom variable: company maps to the native
// {{COMPANY}} field. Custom variable names must not collide with native fields.

import fs from "node:fs";
import { required, parseArgs, readCsv, sleep } from "./_lib.ts";

const BASE = required("EMAILBISON_BASE_URL").replace(/\/+$/, "");
const KEY = required("EMAILBISON_API_KEY");
const NATIVE = ["email", "first_name", "last_name", "company", "company_name", "title", "phone", "website"];

async function api(path: string, method = "GET", body?: any): Promise<any> {
  for (let a = 0; a < 5; a++) {
    const r = await fetch(`${BASE}${path}`, {
      method,
      headers: { Authorization: `Bearer ${KEY}`, Accept: "application/json", "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (r.status === 429 || r.status >= 500) { await sleep(1000 * Math.pow(2, a)); continue; }
    const text = await r.text();
    let j: any; try { j = JSON.parse(text); } catch { j = {}; }
    if (!r.ok) throw new Error(`${r.status} ${method} ${path}: ${text.slice(0, 300)}`);
    if (j?.data && !Array.isArray(j.data) && j.data.success === false) {
      throw new Error(`API rejected the call (auth or workspace mismatch): ${j.data.message ?? "unknown"}`);
    }
    return j;
  }
  throw new Error(`retry-exhausted: ${method} ${path}`);
}

function toLead(row: Record<string, string>): any {
  const lead: any = {
    email: String(row.email ?? "").trim().toLowerCase(),
    first_name: row.first_name ?? "",
    last_name: row.last_name ?? "",
    company_name: row.company_name ?? row.company ?? "",
    title: row.title ?? "",
  };
  if (row.phone) lead.phone = row.phone;
  if (row.website) lead.website = row.website;
  // Everything else rides along as custom variables: MUST be [{name, value}] array,
  // names UPPERCASE. A flat object 422s and the failure silently empties campaigns.
  const custom = Object.entries(row)
    .filter(([k, v]) => !NATIVE.includes(k.toLowerCase()) && String(v ?? "").trim() !== "")
    .map(([k, v]) => ({ name: k.toUpperCase().replace(/[^A-Z0-9_]/g, "_"), value: String(v) }));
  if (custom.length) lead.custom_variables = custom;
  return lead;
}

async function main() {
  const { flags } = parseArgs();
  const campaignId = flags["campaign-id"] as string;
  const csvPath = flags.csv as string;
  if (!campaignId || !csvPath || !fs.existsSync(csvPath)) {
    console.error("Usage: --campaign-id <id> --csv <leads.csv>");
    process.exit(1);
  }

  const rows = readCsv(csvPath).filter(r => String(r.email ?? "").includes("@"));
  const seen = new Set<string>();
  const leads = rows.map(toLead).filter(l => !seen.has(l.email) && seen.add(l.email));
  console.log(`${leads.length} unique leads to upload.`);

  // 1) Bulk upsert in batches (~140 per call is the proven safe size).
  const ids: number[] = [];
  for (let i = 0; i < leads.length; i += 140) {
    const batch = leads.slice(i, i + 140);
    const res = await api("/api/leads/create-or-update/multiple", "POST", { leads: batch });
    const data = Array.isArray(res?.data) ? res.data : [];
    for (const l of data) if (l?.id) ids.push(l.id);
    // Fallback: if the response didn't echo ids, look each lead up by email.
    if (data.length === 0) {
      for (const l of batch) {
        const found = await api(`/api/leads?search=${encodeURIComponent(l.email)}`);
        const hit = (Array.isArray(found?.data) ? found.data : []).find((x: any) => x.email?.toLowerCase() === l.email);
        if (hit?.id) ids.push(hit.id);
        await sleep(100);
      }
    }
    process.stderr.write(`\r upserted ${Math.min(i + 140, leads.length)}/${leads.length} (ids: ${ids.length})   `);
    await sleep(500);
  }
  console.log();

  // 2) Attach to the campaign in batches of 500. Idempotent; bounced leads 422 and are skipped.
  let attached = 0;
  for (let i = 0; i < ids.length; i += 500) {
    const batch = ids.slice(i, i + 500);
    try {
      await api(`/api/campaigns/${campaignId}/leads/attach-leads`, "POST", { lead_ids: batch });
      attached += batch.length;
    } catch (e: any) {
      console.log(`\n  Batch attach warning: ${String(e.message).slice(0, 160)}`);
    }
    process.stderr.write(`\r attached ${Math.min(i + 500, ids.length)}/${ids.length}   `);
    await sleep(200);
  }

  // 3) Verify by reading the campaign's own count back.
  const campaign = (await api(`/api/campaigns/${campaignId}`))?.data ?? {};
  console.log(`\n\nDone. Upserted ${ids.length}, attached ~${attached}.`);
  console.log(`Campaign now reports total_leads: ${campaign.total_leads ?? "?"} (verify this matches what you expect).`);
  console.log(`Next: attach senders if you haven't, then /pre-launch-qa.`);
}

main().catch(e => { console.error(`\n${e.message}`); process.exit(1); });
