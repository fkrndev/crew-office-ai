#!/usr/bin/env tsx
// Generate candidate sending domains and check Dynadot availability.
// Adapted from Eric Nowoslawski's open-source cold outbound skills (MIT).
//
// Run: npx tsx scripts/dynadot-generate-domains.ts --brand acme --count 20
//
// Sending domains are variations of your brand (getacme.com, tryacme.com, acmehq.com),
// never your real domain. Default TLD is .com; fall back to .co when .com is taken out.
// Outputs: generated-domains.csv (domain, available, price). No purchase happens here.

import { required, parseArgs, writeCsv, sleep, retry } from "./_lib.ts";

const PREFIXES = [
  "go", "get", "try", "my", "the", "hey", "use", "run", "hello",
  "meet", "find", "join", "with", "one", "via", "top",
];
const SUFFIXES = [
  "hq", "hub", "app", "pro", "lab", "now", "team", "desk",
  "base", "core", "edge", "flow", "point", "mail", "reach",
];

async function checkBatch(domains: string[], apiKey: string): Promise<any[]> {
  const url = new URL("https://api.dynadot.com/api3.json");
  url.searchParams.set("key", apiKey);
  url.searchParams.set("command", "search");
  url.searchParams.set("show_price", "1");
  url.searchParams.set("currency", "USD");
  domains.forEach((d, i) => url.searchParams.set(`domain${i}`, d));
  const res = await retry(() => fetch(url.toString()).then(r => r.json() as Promise<any>));
  const results = res?.SearchResponse?.SearchResults ?? [];
  return results.map((r: any) => ({
    domain: r.DomainName,
    available: r.Available ?? "no",
    price: r.Price ?? "",
  }));
}

async function main() {
  const { flags } = parseArgs();
  const brand = (flags.brand as string) || "";
  const count = parseInt((flags.count as string) || "20");
  const tld = (flags.tld as string) || "com";
  const maxPrice = parseFloat((flags["max-price"] as string) || "15");

  if (!brand) {
    console.error('Usage: --brand <keyword> [--count 20] [--tld com] [--max-price 15]');
    process.exit(1);
  }
  const apiKey = required("DYNADOT_API_KEY");

  const candidates = new Set<string>();
  for (const p of PREFIXES) candidates.add(`${p}${brand}.${tld}`);
  for (const s of SUFFIXES) candidates.add(`${brand}${s}.${tld}`);
  const list = Array.from(candidates).filter(d => d.length <= 30);

  console.log(`Generated ${list.length} candidates for "${brand}" on .${tld}. Checking Dynadot availability...\n`);

  const all: any[] = [];
  for (let i = 0; i < list.length; i += 100) {
    try {
      all.push(...await checkBatch(list.slice(i, i + 100), apiKey));
    } catch (e: any) {
      console.error(`Batch failed: ${e.message}`);
    }
    await sleep(1000);
  }

  const available = all
    .filter(r => r.available === "yes" && parseFloat(r.price || "999") <= maxPrice)
    .sort((a, b) => a.domain.length - b.domain.length)
    .slice(0, count);

  if (available.length === 0) {
    console.log(`Nothing available under $${maxPrice} on .${tld}.`);
    console.log(tld === "com" ? `Try again with --tld co (the standard fallback).` : `Try a different brand keyword or raise --max-price.`);
    process.exit(1);
  }

  console.log(`${available.length} available under $${maxPrice}:\n`);
  available.forEach(r => console.log(`  ${r.domain.padEnd(30)} $${r.price}`));
  const total = available.reduce((s, r) => s + parseFloat(r.price || "0"), 0);
  console.log(`\nTotal if all purchased: $${total.toFixed(2)}`);

  writeCsv("generated-domains.csv", available);
  console.log(`\nSaved to generated-domains.csv. Review it, remove anything you don't like, then:`);
  console.log(`  npx tsx scripts/dynadot-bulk-purchase.ts --list generated-domains.csv`);
}

main().catch(e => { console.error(e); process.exit(1); });
