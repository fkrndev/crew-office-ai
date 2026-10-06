#!/usr/bin/env tsx
// Bulk-purchase domains from Dynadot. SPENDS REAL MONEY: always confirms first.
// Adapted from Eric Nowoslawski's open-source cold outbound skills (MIT).
//
// Run: npx tsx scripts/dynadot-bulk-purchase.ts --list generated-domains.csv
// Outputs: purchased-domains.csv

import fs from "node:fs";
import { required, parseArgs, readCsv, writeCsv, sleep, confirm, retry } from "./_lib.ts";

async function walletBalance(apiKey: string): Promise<number> {
  const r = await fetch(`https://api.dynadot.com/api3.json?key=${apiKey}&command=account_info`);
  const j: any = await r.json();
  const raw = j?.AccountInfoResponse?.AccountInfo?.AccountBalance ?? "$0.00";
  return parseFloat(String(raw).replace("$", ""));
}

async function register(domain: string, apiKey: string): Promise<{ success: boolean; error?: string }> {
  try {
    const url = `https://api.dynadot.com/api3.json?key=${apiKey}&command=register&domain=${encodeURIComponent(domain)}&duration=1`;
    const r = await retry(() => fetch(url).then(x => x.json() as Promise<any>));
    if (r?.RegisterResponse?.ResponseCode === 0) return { success: true };
    return { success: false, error: r?.RegisterResponse?.Error ?? "unknown" };
  } catch (e: any) {
    return { success: false, error: e.message };
  }
}

async function main() {
  const { flags } = parseArgs();
  const listPath = (flags.list as string) || "generated-domains.csv";
  if (!fs.existsSync(listPath)) {
    console.error(`No CSV at ${listPath}. Run dynadot-generate-domains.ts first, or pass --list <file>.`);
    process.exit(1);
  }
  const apiKey = required("DYNADOT_API_KEY");

  const rows = readCsv(listPath).filter(r => r.available === "yes");
  if (rows.length === 0) {
    console.error("No available domains in the input CSV.");
    process.exit(1);
  }

  const totalCost = rows.reduce((s, r) => s + parseFloat(r.price || "0"), 0);
  const balance = await walletBalance(apiKey);
  console.log(`${rows.length} domains, total $${totalCost.toFixed(2)}. Dynadot wallet: $${balance.toFixed(2)}.\n`);

  if (balance < totalCost) {
    console.error(`Insufficient balance. Top up at least $${(totalCost - balance).toFixed(2)} at https://www.dynadot.com/account`);
    process.exit(1);
  }

  const ok = await confirm(`About to spend $${totalCost.toFixed(2)} on ${rows.length} domains. Confirm?`);
  if (!ok) { console.log("Cancelled. Nothing spent."); process.exit(0); }

  const results: any[] = [];
  for (const [i, d] of rows.entries()) {
    process.stdout.write(`[${i + 1}/${rows.length}] ${d.domain} ... `);
    const r = await register(d.domain, apiKey);
    console.log(r.success ? "✅" : `❌ ${r.error}`);
    results.push({ domain: d.domain, status: r.success ? "purchased" : "failed", price: d.price, error: r.error ?? "" });
    await sleep(500);
  }
  writeCsv("purchased-domains.csv", results);

  const bought = results.filter(r => r.status === "purchased").length;
  console.log(`\n✅ Purchased ${bought}, failed ${results.length - bought}. Saved to purchased-domains.csv.`);
  console.log(`Next: npx tsx scripts/zapmail-setup.ts --domains purchased-domains.csv`);
}

main().catch(e => { console.error(e); process.exit(1); });
