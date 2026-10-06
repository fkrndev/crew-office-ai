#!/usr/bin/env tsx
// Create an EmailBison campaign as a DRAFT: sequence steps + variants, schedule, settings.
//
// This script NEVER activates anything. That's deliberate. You run /pre-launch-qa
// and press Start yourself in the EmailBison UI.
//
// Usage:
//   npx tsx scripts/emailbison-create-campaign.ts --name "Q3 SaaS founders" --steps steps.json \
//       [--type outbound] [--timezone America/New_York] [--start 09:00] [--end 17:00]
//
// steps.json: one entry per sequence step; first variant is the primary, the rest A/B:
// [
//   { "title": "Step 1", "wait_in_days": 1,
//     "variants": [ { "subject": "quick question", "body": "..." },
//                   { "subject": "idea for {{COMPANY}}", "body": "..." } ] },
//   { "title": "Step 2", "wait_in_days": 3, "thread_reply": true,
//     "variants": [ { "subject": "", "body": "..." } ] }
// ]
// Rules: wait_in_days >= 3 between real steps; thread_reply steps get a BARE subject
// (the platform adds "Re:" itself); variables are {{DOUBLE_CURLY_UPPERCASE}}.

import fs from "node:fs";
import { env, required, parseArgs, sleep } from "./_lib.ts";

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
    // EmailBison auth failures are 200-shaped: {"data":{"success":false,...}}
    if (j?.data && !Array.isArray(j.data) && j.data.success === false) {
      throw new Error(`API rejected the call (auth or workspace mismatch): ${j.data.message ?? "unknown"}`);
    }
    return j;
  }
  throw new Error(`retry-exhausted: ${method} ${path}`);
}

async function main() {
  const { flags } = parseArgs();
  const name = flags.name as string;
  const stepsPath = flags.steps as string;
  const type = (flags.type as string) || "outbound";
  if (!name || !stepsPath || !fs.existsSync(stepsPath)) {
    console.error(`Usage: --name "Campaign name" --steps steps.json [--type outbound|reply_followup]`);
    process.exit(1);
  }
  const steps: any[] = JSON.parse(fs.readFileSync(stepsPath, "utf8"));

  if (!Array.isArray(steps) || steps.length === 0) {
    console.error(`${stepsPath} has no steps. A campaign needs at least one step with at least one variant.`);
    process.exit(1);
  }
  for (const [i, s] of steps.entries()) {
    if (!Array.isArray(s.variants) || s.variants.length === 0) {
      console.error(`Step ${i + 1} ("${s.title ?? "untitled"}") has no variants. Every step needs at least one. Fix steps.json.`);
      process.exit(1);
    }
    if (i > 0 && Number(s.wait_in_days ?? 0) < 3) {
      console.error(`Step ${i + 1} has wait_in_days ${s.wait_in_days}. Minimum gap between steps is 3 days. Fix steps.json.`);
      process.exit(1);
    }
  }

  console.log(`Creating DRAFT campaign "${name}" (type: ${type})...`);
  const created = await api("/api/campaigns", "POST", { name, type });
  const campaign = created?.data ?? created;
  const id = campaign?.id;
  if (!id) throw new Error(`No campaign id in response: ${JSON.stringify(created).slice(0, 300)}`);
  console.log(`  Campaign created (id ${id}), status: ${campaign?.status ?? "draft"}`);

  // Sequence steps: ALL variants of a step in one POST. First = primary, rest variant:true.
  for (const [i, s] of steps.entries()) {
    const sequence_steps = s.variants.map((v: any, vi: number) => ({
      email_subject: v.subject ?? "",
      email_body: v.body ?? "",
      wait_in_days: Number(s.wait_in_days ?? (i === 0 ? 1 : 3)),
      ...(s.thread_reply !== undefined ? { thread_reply: !!s.thread_reply } : {}),
      ...(vi > 0 ? { variant: true } : {}),
    }));
    await api(`/api/campaigns/${id}/sequence-steps`, "POST", { title: s.title ?? `Step ${i + 1}`, sequence_steps });
    console.log(`  Step ${i + 1}: ${s.variants.length} variant(s) uploaded`);
  }

  // Schedule (a hard prerequisite for the campaign ever starting). save_as_template is REQUIRED.
  const timezone = (flags.timezone as string) || "America/New_York";
  await api(`/api/campaigns/${id}/schedule`, "POST", {
    monday: true, tuesday: true, wednesday: true, thursday: true, friday: true,
    saturday: false, sunday: false,
    start_time: (flags.start as string) || "09:00",
    end_time: (flags.end as string) || "17:00",
    timezone,
    save_as_template: false,
  });
  console.log(`  Schedule set: Mon-Fri, ${(flags.start as string) || "09:00"}-${(flags.end as string) || "17:00"} ${timezone}`);

  // Settings. EmailBison PATCH resets EVERY unspecified boolean, so all five that matter
  // here get sent every time, never just the ones you meant to change.
  // plain_text: outbound campaigns send plain text; reply_followup stays HTML so links stay clickable.
  // can_unsubscribe: false, always. This system's opt-out is the reply-instruction line baked
  // into the copy itself (see /reply-classification, pre-launch-qa 1.6). Setting this true makes
  // the platform ALSO append its own unsubscribe mechanism: two opt-out paths, a real failure mode.
  // reputation_building: true, always. Matches the warmup-stays-on-forever doctrine everywhere
  // else in this system; never disable reputation protection to chase short-term volume.
  const settings = {
    max_emails_per_day: 1000,
    max_new_leads_per_day: 1000,
    plain_text: type === "outbound",
    open_tracking: false,
    can_unsubscribe: false,
    reputation_building: true,
    include_auto_replies_in_stats: false,
  };
  await api(`/api/campaigns/${id}/update`, "PATCH", settings);
  const after = (await api(`/api/campaigns/${id}`))?.data ?? {};
  console.log(`  Settings applied. Verify: plain_text=${after.plain_text}, open_tracking=${after.open_tracking}, can_unsubscribe=${after.can_unsubscribe}, limits=${after.max_emails_per_day}/day`);

  console.log(`\nDone. Campaign ${id} is a DRAFT with copy, schedule, and settings.`);
  console.log(`Next:`);
  console.log(`  1. npx tsx scripts/emailbison-upload-leads.ts --campaign-id ${id} --csv sendable-leads.csv`);
  console.log(`  2. Attach your sender emails (UI, or /emailbison-api for the endpoint)`);
  console.log(`  3. Run /pre-launch-qa`);
  console.log(`  4. Press Start yourself in EmailBison. This tooling never launches for you.`);
}

main().catch(e => { console.error(`\n${e.message}`); process.exit(1); });
