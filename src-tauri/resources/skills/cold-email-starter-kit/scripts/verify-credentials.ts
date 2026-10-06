#!/usr/bin/env tsx
// Verify every configured API key with a cheap, safe call.
// Usage:
//   npm run verify              — checks everything configured, spends nothing
//   npm run verify -- --live    — also runs ONE real Kitt verification (~1 credit)
//
// Core services (AI Ark, EmailBison) fail the run if missing or broken.
// Infrastructure and verification keys are optional: skip freely if you
// already have inboxes, or aren't importing your own lists.

import { env, parseArgs, printTable } from "./_lib.ts";

type Check = { service: string; status: "✅" | "⚠️" | "❌"; notes: string };

const { flags } = parseArgs();

async function checkAiArk(): Promise<Check> {
  if (!env.AI_ARK_API_KEY) return { service: "AI Ark", status: "❌", notes: "AI_ARK_API_KEY not set (required for list building)" };
  try {
    const r = await fetch("https://api.ai-ark.com/api/developer-portal/v1/payments/credits", {
      headers: { "X-TOKEN": env.AI_ARK_API_KEY, "Content-Type": "application/json" },
    });
    if (!r.ok) return { service: "AI Ark", status: "❌", notes: `HTTP ${r.status} — key rejected?` };
    const j: any = await r.json();
    const credits = j?.total ?? j?.credits?.total ?? j?.credits ?? JSON.stringify(j).slice(0, 60);
    return { service: "AI Ark", status: "✅", notes: `Credits: ${credits}` };
  } catch (e: any) {
    return { service: "AI Ark", status: "❌", notes: `Network error: ${e.message}` };
  }
}

async function checkEmailBison(): Promise<Check> {
  if (!env.EMAILBISON_BASE_URL || !env.EMAILBISON_API_KEY) {
    return { service: "EmailBison", status: "❌", notes: "EMAILBISON_BASE_URL and EMAILBISON_API_KEY both required" };
  }
  const base = env.EMAILBISON_BASE_URL.replace(/\/+$/, "");
  try {
    const r = await fetch(`${base}/api/sender-emails?page=1`, {
      headers: { Authorization: `Bearer ${env.EMAILBISON_API_KEY}`, Accept: "application/json" },
    });
    if (!r.ok) return { service: "EmailBison", status: "❌", notes: `HTTP ${r.status}` };
    const j: any = await r.json();
    // EmailBison auth failures come back 200-shaped with data as an OBJECT.
    // A real result always has data as an array.
    if (!Array.isArray(j?.data)) {
      const msg = j?.data?.message ?? "key rejected or wrong workspace";
      return { service: "EmailBison", status: "❌", notes: `Auth-shaped failure: ${String(msg).slice(0, 60)}` };
    }
    const total = j?.meta?.total ?? j.data.length;
    return { service: "EmailBison", status: "✅", notes: `${total} sender email(s) in workspace` };
  } catch (e: any) {
    return { service: "EmailBison", status: "❌", notes: `Network error: ${e.message} (check EMAILBISON_BASE_URL)` };
  }
}

async function checkDynadot(): Promise<Check> {
  if (!env.DYNADOT_API_KEY) return { service: "Dynadot", status: "⚠️", notes: "not set (only needed to buy domains)" };
  try {
    const r = await fetch(`https://api.dynadot.com/api3.json?key=${env.DYNADOT_API_KEY}&command=account_info`);
    const j: any = await r.json();
    if (j.Response?.ResponseCode === 0 || j.AccountInfoResponse) {
      const balance = j.AccountInfoResponse?.AccountInfo?.AccountBalance ?? "unknown";
      return { service: "Dynadot", status: "✅", notes: `Wallet: ${balance}` };
    }
    return { service: "Dynadot", status: "❌", notes: `API error: ${j.Error ?? JSON.stringify(j).slice(0, 60)}` };
  } catch (e: any) {
    return { service: "Dynadot", status: "❌", notes: `Network error: ${e.message}` };
  }
}

async function checkZapmail(): Promise<Check> {
  if (!env.ZAPMAIL_API_KEY) return { service: "Zapmail", status: "⚠️", notes: "not set (only needed to create inboxes)" };
  try {
    const r = await fetch("https://api.zapmail.ai/api/v2/domains/assignable?limit=5&page=1", {
      headers: { "x-auth-zapmail": env.ZAPMAIL_API_KEY },
    });
    if (r.status !== 200) return { service: "Zapmail", status: "❌", notes: `HTTP ${r.status}` };
    const j: any = await r.json();
    return { service: "Zapmail", status: "✅", notes: `${j?.data?.length ?? 0} assignable domain(s) visible` };
  } catch (e: any) {
    return { service: "Zapmail", status: "❌", notes: `Network error: ${e.message}` };
  }
}

async function checkKitt(): Promise<Check> {
  if (!env.KITT_API_KEY) return { service: "Kitt", status: "⚠️", notes: "not set (only needed to verify lists you bring yourself)" };
  if (!flags.live) {
    return { service: "Kitt", status: "✅", notes: "key set (every Kitt call costs a credit; add --live to test with 1 real verification)" };
  }
  try {
    const r = await fetch("https://api.trykitt.ai/job/verify_email", {
      method: "POST",
      headers: { "x-api-key": env.KITT_API_KEY, "Content-Type": "application/json" },
      body: JSON.stringify({ email: "postmaster@gmail.com", realtime: true, treatAliasesAsValid: true }),
    });
    const body = await r.text();
    if (!r.ok) {
      const funds = /fund|credit|balance/i.test(body);
      return { service: "Kitt", status: "❌", notes: funds ? "out of credits" : `HTTP ${r.status}` };
    }
    const j: any = JSON.parse(body);
    return { service: "Kitt", status: "✅", notes: `live check OK (verdict: ${j?.validity ?? j?.displayText ?? "?"})` };
  } catch (e: any) {
    return { service: "Kitt", status: "❌", notes: `Network error: ${e.message}` };
  }
}

async function main() {
  console.log("Checking credentials...\n");
  const checks = await Promise.all([checkAiArk(), checkEmailBison(), checkDynadot(), checkZapmail(), checkKitt()]);
  printTable(checks as any);
  console.log();

  const coreFailed = checks.filter(c => (c.service === "AI Ark" || c.service === "EmailBison") && c.status === "❌");
  const optionalFailed = checks.filter(c => c.status === "❌" && !coreFailed.includes(c));

  if (coreFailed.length > 0) {
    console.log(`❌ Core service(s) failing: ${coreFailed.map(c => c.service).join(", ")}. Fix these before continuing — nothing works without them.`);
    process.exit(1);
  }
  if (optionalFailed.length > 0) {
    console.log(`⚠️ Optional service(s) failing: ${optionalFailed.map(c => c.service).join(", ")}. Fine to ignore unless you need those skills.`);
  }
  console.log("✅ Core credentials look good. Next: /cold-email-kickoff");
}

main();
