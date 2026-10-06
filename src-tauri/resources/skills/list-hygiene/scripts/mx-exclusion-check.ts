#!/usr/bin/env tsx
// Strip leads whose domains route mail through Mimecast or Barracuda.
//
// Those security gateways quarantine cold email and poison sender reputation.
// Excluding them is automatic list hygiene, not a judgment call. Free to run:
// it's just DNS lookups.
//
// Usage:
//   npx tsx scripts/mx-exclusion-check.ts --csv leads.csv [--email-column email]
//
// Output:
//   sendable-leads.csv    rows whose domains are safe to send to
//   excluded-leads.csv    rows stripped, with the matching MX shown

import fs from "node:fs";
import dns from "node:dns/promises";
import { parseArgs, readCsv, writeCsv, createQueue } from "./_lib.ts";

const BAD = ["mimecast", "barracuda"];

async function mxHosts(domain: string): Promise<string> {
  try {
    const records = await dns.resolveMx(domain);
    return records.map(r => r.exchange.toLowerCase()).join(" ");
  } catch {
    return ""; // no MX / NXDOMAIN: not our problem here, verification handles dead domains
  }
}

function domainOf(row: Record<string, string>, col: string): string {
  const email = String(row[col] ?? "").trim().toLowerCase();
  return email.includes("@") ? email.split("@")[1] : "";
}

async function main() {
  const { flags } = parseArgs();
  const csvPath = flags.csv as string;
  const col = (flags["email-column"] as string) || "email";
  if (!csvPath || !fs.existsSync(csvPath)) {
    console.error("Usage: --csv <leads.csv> [--email-column email]");
    process.exit(1);
  }

  const rows = readCsv(csvPath);
  if (rows.length && rows[0][col] === undefined) {
    console.error(`Column "${col}" not found. Columns: ${Object.keys(rows[0]).join(", ")}`);
    process.exit(1);
  }

  const domains = Array.from(new Set(rows.map(r => domainOf(r, col)).filter(Boolean)));
  console.log(`${rows.length} rows, ${domains.length} unique domains. Resolving MX records...`);

  const queue = createQueue(20);
  const badDomains = new Map<string, string>();
  let checked = 0;
  await Promise.all(domains.map(d => queue.add(async () => {
    const mx = await mxHosts(d);
    const hit = BAD.find(b => mx.includes(b));
    if (hit) badDomains.set(d, mx.split(" ").find(h => h.includes(hit)) ?? hit);
    checked++;
    if (checked % 200 === 0) process.stderr.write(`\r checked ${checked}/${domains.length}, ${badDomains.size} protected   `);
  })));

  const sendable: Record<string, string>[] = [];
  const excluded: Record<string, string>[] = [];
  let noEmail = 0;
  for (const r of rows) {
    const d = domainOf(r, col);
    if (!d) { excluded.push({ ...r, matched_mx: "(no valid email)" }); noEmail++; continue; }
    if (badDomains.has(d)) excluded.push({ ...r, matched_mx: badDomains.get(d)! });
    else sendable.push(r);
  }

  writeCsv("sendable-leads.csv", sendable);
  writeCsv("excluded-leads.csv", excluded);
  console.log(`\n\nSendable: ${sendable.length} → sendable-leads.csv`);
  console.log(`Excluded: ${excluded.length} → excluded-leads.csv (${excluded.length - noEmail} Mimecast/Barracuda, ${noEmail} missing or invalid email)`);
  console.log(`\nNext: /emailbison-campaign-launch with sendable-leads.csv.`);
}

main().catch(e => { console.error(e.message); process.exit(1); });
