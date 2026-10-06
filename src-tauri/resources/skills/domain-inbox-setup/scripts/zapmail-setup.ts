#!/usr/bin/env tsx
// Zapmail setup: switch nameservers, connect domains, create inboxes.
// Adapted from Eric Nowoslawski's open-source cold outbound skills (MIT),
// with the final hop pointed at EmailBison instead of Smartlead.
//
// Run: npx tsx scripts/zapmail-setup.ts --domains purchased-domains.csv
//
// Phases: 1) Dynadot NS switch → 2) DNS propagation wait → 3) Zapmail connect +
// poll assignable → 4) create 2 inboxes per domain. Provisioning then takes
// 4-6 hours; connect the finished inboxes to EmailBison afterwards.

import fs from "node:fs";
import { env, required, parseArgs, readCsv, writeCsv, sleep, retry, confirm } from "./_lib.ts";

const ZAPMAIL_NS = ["pns61.cloudns.net", "pns62.cloudns.com", "pns63.cloudns.net", "pns64.cloudns.uk"];

// NOTE: this sends one comma-joined `domain` param covering the whole batch, inherited from
// the source this was adapted from. Dynadot's docs describe set_ns as single-domain elsewhere,
// so this bulk form is unverified against a live account by this repo. After Phase 1 runs,
// spot-check 1-2 domains in your Dynadot dashboard before waiting out DNS propagation, so a
// silent partial failure surfaces in a minute instead of 20.
async function dynadotSetNs(domains: string[], apiKey: string): Promise<void> {
  const params = new URLSearchParams();
  params.set("key", apiKey);
  params.set("command", "set_ns");
  params.set("domain", domains.join(","));
  ZAPMAIL_NS.forEach((ns, i) => params.set(`ns${i}`, ns));
  const url = `https://api.dynadot.com/api3.json?${params.toString().replace(/%2C/g, ",")}`;
  const r = await retry(() => fetch(url).then(x => x.json() as Promise<any>));
  if (r?.SetNsResponse?.ResponseCode !== 0) throw new Error(`Dynadot set_ns failed: ${JSON.stringify(r).slice(0, 200)}`);
}

async function zapmailConnect(domainNames: string[], apiKey: string): Promise<void> {
  for (let i = 0; i < domainNames.length; i += 50) {
    const res = await retry(() => fetch("https://api.zapmail.ai/api/v2/domains/connect-domain", {
      method: "POST",
      headers: { "x-auth-zapmail": apiKey, "Content-Type": "application/json" },
      body: JSON.stringify({ domainNames: domainNames.slice(i, i + 50) }),
    }));
    if (!res.ok) throw new Error(`Zapmail connect failed: ${await res.text()}`);
    await sleep(3000);
  }
}

async function waitUntilAssignable(domainNames: string[], apiKey: string, timeoutMinutes = 30): Promise<Array<{ domain: string; uuid: string }>> {
  const deadline = Date.now() + timeoutMinutes * 60 * 1000;
  const target = new Set(domainNames);
  const found: Array<{ domain: string; uuid: string }> = [];
  const seen = new Set<string>();

  while (Date.now() < deadline) {
    for (let page = 1; page < 100; page++) {
      const res = await retry(() => fetch(`https://api.zapmail.ai/api/v2/domains/assignable?limit=100&page=${page}`, {
        headers: { "x-auth-zapmail": apiKey },
      }));
      if (!res.ok) break;
      const j: any = await res.json();
      const items: any[] = j?.data ?? [];
      if (items.length === 0) break;
      for (const item of items) {
        if (target.has(item.domainName) && !seen.has(item.domainName)) {
          found.push({ domain: item.domainName, uuid: item.uuid });
          seen.add(item.domainName);
        }
      }
      if (items.length < 100) break;
    }
    if (found.length / domainNames.length >= 0.95) {
      console.log(`${found.length}/${domainNames.length} assignable. Proceeding.`);
      return found;
    }
    console.log(`${found.length}/${domainNames.length} assignable, waiting 5 min...`);
    await sleep(5 * 60 * 1000);
  }
  console.warn(`Timeout: ${found.length}/${domainNames.length} assignable. Continuing with what's ready.`);
  return found;
}

const INBOX_STATE_FILE = ".zapmail-inbox-state.json";

function loadInboxState(): Record<string, boolean> {
  return fs.existsSync(INBOX_STATE_FILE) ? JSON.parse(fs.readFileSync(INBOX_STATE_FILE, "utf8")) : {};
}
function saveInboxState(state: Record<string, boolean>): void {
  fs.writeFileSync(INBOX_STATE_FILE, JSON.stringify(state, null, 2));
}

async function createSingleMailboxPair(
  uuid: string, domain: string, firstName: string, lastName: string, prefix1: string, prefix2: string, apiKey: string,
): Promise<boolean> {
  try {
    const single = { [uuid]: [
      { firstName, lastName, mailboxUsername: prefix1, domainName: domain },
      { firstName, lastName, mailboxUsername: prefix2, domainName: domain },
    ] };
    const r = await fetch("https://api.zapmail.ai/api/v2/mailboxes", {
      method: "POST",
      headers: { "x-auth-zapmail": apiKey, "Content-Type": "application/json" },
      body: JSON.stringify(single),
    });
    if (!r.ok) console.warn(`  ${domain} failed: ${r.status}`);
    return r.ok;
  } catch (e: any) {
    console.warn(`  ${domain} error: ${e.message}`);
    return false;
  }
}

// Billable and resumable: state is persisted to disk after every batch, so a
// killed or crashed run can be re-run without recreating (and re-billing)
// domains that already succeeded.
async function createMailboxes(
  assignable: Array<{ domain: string; uuid: string }>,
  firstName: string, lastName: string, prefix1: string, prefix2: string, apiKey: string,
): Promise<Record<string, boolean>> {
  const results = loadInboxState();
  const todo = assignable.filter(({ domain }) => !results[domain]);
  if (todo.length < assignable.length) {
    console.log(`  Resuming: ${assignable.length - todo.length} domain(s) already have inboxes per ${INBOX_STATE_FILE}, skipping those.`);
  }

  for (let i = 0; i < todo.length; i += 25) {
    const batch = todo.slice(i, i + 25);
    const body: Record<string, any> = {};
    for (const { uuid, domain } of batch) {
      body[uuid] = [
        { firstName, lastName, mailboxUsername: prefix1, domainName: domain },
        { firstName, lastName, mailboxUsername: prefix2, domainName: domain },
      ];
    }

    let batchOk = false;
    try {
      const res = await fetch("https://api.zapmail.ai/api/v2/mailboxes", {
        method: "POST",
        headers: { "x-auth-zapmail": apiKey, "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      batchOk = res.ok;
      if (!res.ok) console.warn(`Batch failed (${res.status}), retrying one at a time...`);
    } catch (e: any) {
      console.warn(`Batch request failed (${e.message}), retrying one at a time...`);
    }

    if (batchOk) {
      batch.forEach(({ domain }) => { results[domain] = true; });
    } else {
      // Per-domain fallback is itself wrapped in try/catch, so one bad domain
      // can't take down the rest of the batch or lose already-confirmed state.
      for (const { uuid, domain } of batch) {
        results[domain] = await createSingleMailboxPair(uuid, domain, firstName, lastName, prefix1, prefix2, apiKey);
        await sleep(500);
      }
    }
    saveInboxState(results); // persist after EVERY batch, not just at the end
    await sleep(3000);
  }
  return results;
}

async function main() {
  const { flags } = parseArgs();
  const domainsFile = (flags.domains as string) || "purchased-domains.csv";
  const firstName = (flags["first-name"] as string) || env.SENDER_FIRST_NAME || "";
  const lastName = (flags["last-name"] as string) || env.SENDER_LAST_NAME || "";
  if (!firstName || !lastName) {
    console.error("Set SENDER_FIRST_NAME and SENDER_LAST_NAME in .env (or pass --first-name/--last-name). Inboxes are named after the sender.");
    process.exit(1);
  }
  const prefix1 = firstName.toLowerCase();
  const prefix2 = `${firstName.toLowerCase()}${lastName.toLowerCase()}`.slice(0, 20);
  const skipWait = !!flags["skip-wait"];

  const dynadotKey = required("DYNADOT_API_KEY");
  const zapmailKey = required("ZAPMAIL_API_KEY");

  if (!fs.existsSync(domainsFile)) {
    console.error(`No CSV at ${domainsFile}. Run dynadot-bulk-purchase.ts first, or pass --domains <file>.`);
    process.exit(1);
  }
  const domains = readCsv(domainsFile).filter(r => r.status === "purchased").map(r => r.domain);
  if (domains.length === 0) {
    console.error(`No purchased domains in ${domainsFile}.`);
    process.exit(1);
  }

  console.log(`Zapmail setup for ${domains.length} domains. Sender: ${firstName} ${lastName}.`);
  console.log(`Inboxes per domain: ${prefix1}@... and ${prefix2}@... (${domains.length * 2} total)\n`);

  console.log("Phase 1/4: switching nameservers on Dynadot...");
  for (let i = 0; i < domains.length; i += 100) await dynadotSetNs(domains.slice(i, i + 100), dynadotKey);
  console.log("✅ API reports nameservers switched. Spot-check 1-2 domains in your Dynadot dashboard now, before the 20-minute wait.\n");

  if (!skipWait) {
    console.log("Phase 2/4: waiting 20 minutes for DNS propagation (or re-run later with --skip-wait)...");
    for (let m = 20; m > 0; m--) {
      process.stdout.write(`  ${m} min remaining...\r`);
      await sleep(60 * 1000);
    }
    console.log("  Done.                \n");
  }

  console.log("Phase 3/4: connecting domains on Zapmail...");
  await zapmailConnect(domains, zapmailKey);
  const assignable = await waitUntilAssignable(domains, zapmailKey, 30);
  console.log(`✅ ${assignable.length} domains assignable.\n`);

  // Inboxes are the billable unit on Zapmail, so this is the money gate.
  const inboxCount = assignable.length * 2;
  if (!flags.yes) {
    const ok = await confirm(`Phase 4 creates ${inboxCount} inboxes on ${assignable.length} domains, billed on your Zapmail plan. Continue?`);
    if (!ok) { console.log("Stopped before inbox creation. Re-run when ready (phases 1-3 are done and idempotent)."); process.exit(0); }
  }
  console.log("Phase 4/4: creating inboxes...");
  const results = await createMailboxes(assignable, firstName, lastName, prefix1, prefix2, zapmailKey);
  const rows: any[] = [];
  for (const { domain } of assignable) {
    const status = results[domain] ? "created" : "failed";
    rows.push({ email: `${prefix1}@${domain}`, domain, status });
    rows.push({ email: `${prefix2}@${domain}`, domain, status });
  }
  writeCsv("inboxes.csv", rows);
  console.log(`✅ Inboxes requested on ${Object.values(results).filter(Boolean).length} domains. Saved to inboxes.csv.\n`);

  console.log("Provisioning takes 4-6 hours. When the inboxes show ACTIVE in Zapmail:");
  console.log("  1. In Zapmail, export the mailbox credentials (Zapmail's built-in export targets");
  console.log("     other platforms, so use the credentials export for EmailBison).");
  console.log("  2. In EmailBison: Sender Emails → import/connect the inboxes.");
  console.log("  3. Warm up 2-3 weeks before real volume. Start senders at 1 email/day (/sender-scaling).");
}

main().catch(e => { console.error(e); process.exit(1); });
