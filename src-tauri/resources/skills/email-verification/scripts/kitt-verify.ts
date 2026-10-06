#!/usr/bin/env tsx
// Verify a CSV of emails through Kitt (trykitt.ai). Resumable, capped, patient.
//
// ONLY for lists you brought from elsewhere (scraped, bought, CRM exports).
// AI Ark exports are already verified at source: re-verifying them wastes money.
//
// Usage:
//   npx tsx scripts/kitt-verify.ts --csv leads.csv [--cap 500] [--email-column email]
//
// Output:
//   verified-leads.csv   input rows + a "verdict" column (valid / valid-risky / invalid / ...)
//   .kitt-state.json     verdict cache; re-running skips already-verified emails
//
// Verdicts: valid = usable · valid-risky = catch-all (your call) · invalid = drop.
// Every call costs a credit, so runs are capped (default 500) and resumable.

import fs from "node:fs";
import { required, parseArgs, readCsv, writeCsv, sleep, confirm } from "./_lib.ts";

const KEY = required("KITT_API_KEY");
const STATE_FILE = ".kitt-state.json";
const CONC = 3; // Kitt's realtime API throttles hard under load. 3 is the ceiling; be patient.

type Verdict = { verdict: string; validIdentity?: boolean; validSMTP?: boolean };

async function verify(email: string): Promise<Verdict & { funds?: boolean }> {
  for (let a = 0; a < 12; a++) {
    try {
      const r = await fetch("https://api.trykitt.ai/job/verify_email", {
        method: "POST",
        headers: { "x-api-key": KEY, "Content-Type": "application/json" },
        body: JSON.stringify({ email, realtime: true, treatAliasesAsValid: true }),
      });
      const text = await r.text();
      // Out-of-credits can arrive as a 402 OR a raw 500. Check the body either way,
      // otherwise a dead account burns 12 retries on every remaining address.
      if (!r.ok && /fund|credit|balance/i.test(text)) return { verdict: "NOFUNDS", funds: true };
      if (r.status === 402 || r.status === 429) {
        await sleep(Math.min(20000, 3000 * (a + 1))); // throttled: wait it out
        continue;
      }
      let d: any = {};
      try { d = JSON.parse(text); } catch { /* keep {} */ }
      if (!r.ok) return { verdict: `err:${r.status}` };
      return {
        verdict: String(d.validity ?? d.displayText ?? "unknown"),
        validIdentity: d.validIdentity,
        validSMTP: d.validSMTP,
      };
    } catch {
      await sleep(1000);
    }
  }
  return { verdict: "err:retry" };
}

async function main() {
  const { flags } = parseArgs();
  const csvPath = flags.csv as string;
  const cap = Number(flags.cap ?? 500);
  const col = (flags["email-column"] as string) || "email";
  if (!csvPath || !fs.existsSync(csvPath)) {
    console.error("Usage: --csv <leads.csv> [--cap 500] [--email-column email]");
    process.exit(1);
  }

  const rows = readCsv(csvPath);
  if (rows.length && rows[0][col] === undefined) {
    console.error(`Column "${col}" not found. Columns: ${Object.keys(rows[0]).join(", ")}`);
    process.exit(1);
  }

  const state: Record<string, Verdict> = fs.existsSync(STATE_FILE)
    ? JSON.parse(fs.readFileSync(STATE_FILE, "utf8"))
    : {};

  const todo = rows
    .map(r => String(r[col] ?? "").trim().toLowerCase())
    .filter((e, i, arr) => e && arr.indexOf(e) === i && !state[e])
    .slice(0, cap);

  console.log(`${rows.length} rows in CSV | ${todo.length} unverified to do this run (cap ${cap})`);
  if (!flags.yes) {
    const ok = await confirm(`This will spend up to ${todo.length} Kitt credits. Continue?`);
    if (!ok) { console.log("Cancelled. Nothing spent."); process.exit(0); }
  }
  console.log();

  let done = 0;
  let stop = false;
  for (let i = 0; i < todo.length && !stop; i += CONC) {
    const chunk = todo.slice(i, i + CONC);
    const results = await Promise.all(chunk.map(verify));
    for (let j = 0; j < chunk.length; j++) {
      if (results[j].funds) {
        stop = true;
        console.log("\n!! Kitt out of credits. Stopping. Top up and re-run: already-verified emails are skipped.");
        break;
      }
      const { funds, ...v } = results[j];
      // Never cache transient errors as verdicts: an err:* email retries on the next run.
      if (!v.verdict.startsWith("err:")) state[chunk[j]] = v;
    }
    done += chunk.length;
    fs.writeFileSync(STATE_FILE, JSON.stringify(state)); // persist after every chunk: killable + resumable
    if (done % 30 < CONC) process.stderr.write(`\r verified ${done}/${todo.length}   `);
  }
  fs.writeFileSync(STATE_FILE, JSON.stringify(state));

  const out = rows.map(r => ({
    ...r,
    verdict: state[String(r[col] ?? "").trim().toLowerCase()]?.verdict ?? "",
  }));
  writeCsv("verified-leads.csv", out);

  const counts: Record<string, number> = {};
  for (const r of out) counts[r.verdict || "(not yet run)"] = (counts[r.verdict || "(not yet run)"] ?? 0) + 1;
  console.log(`\n\nWritten: verified-leads.csv`);
  for (const [k, v] of Object.entries(counts).sort((a, b) => b[1] - a[1])) console.log(`  ${k.padEnd(16)} ${v}`);
  console.log(`\nKeep "valid". "valid-risky" is catch-all: include only if you accept some bounces.`);
  console.log(`Next: /list-hygiene before upload.`);
}

main().catch(e => { console.error(e.message); process.exit(1); });
