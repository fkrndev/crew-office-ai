---
name: emailbison-campaign-launch
description: "Assemble a complete EmailBison campaign, DRAFT-only: create the campaign, load every copy variant into one step, upload and attach the leads, attach ALL senders, set the schedule, set the settings that protect deliverability, then STOP. Never activates anything; you press start yourself after /pre-launch-qa. Triggers on 'build the campaign', 'set up the campaign', 'upload the leads', 'launch my campaign', 'get this live', 'put the copy into EmailBison', 'create a campaign in EmailBison', 'ready to send'. Uses scripts/emailbison-create-campaign.ts, scripts/emailbison-upload-leads.ts, scripts/emailbison-pull-analytics.ts."
---

# EmailBison Campaign Launch (build to draft, never to live)

This skill assembles a campaign end to end and then deliberately stops one step short of sending. The campaign stays DRAFT. You run `/pre-launch-qa`, and then you, personally, press start. There is no activation code in any of the scripts here and that's a feature, not a gap: the QA gate only works if nothing can go around it.

## The core rule: ONE campaign per lead list

One list, one campaign. All your offers and copy templates go in as A/B variants **within a single step** of that one campaign. Not one campaign per offer. Not one step per variant. You only create a second campaign when there's a genuinely separate lead list (different segment, different geography).

Why: variants inside one step split-test cleanly against the same audience with shared stats. Separate campaigns per offer fragment your senders, your data, and your ability to read results.

So a build with 3 offers and 3 templates against one list is: 1 campaign, step 1 carrying 9 variants.

## When to use this

- Copy is written and approved, list is built and clean, senders are warm
- "Build the campaign", "upload the leads", "get this into EmailBison"
- Rebuilding a campaign after a rework

Prerequisites, honestly checked: warmed senders in the workspace (`/domain-inbox-setup`, `/sender-scaling`), a hygiene-passed list (`/list-hygiene`), copy with variants ready (`/cold-email-copywriting`, `/sequence-architecture`). If any of those aren't true, do them first; this skill assembles, it doesn't fix inputs.

## The build sequence

Run `${CLAUDE_SKILL_DIR}/scripts/emailbison-create-campaign.ts` for steps 1-3 and 5-6, `${CLAUDE_SKILL_DIR}/scripts/emailbison-upload-leads.ts` for step 4. Every step verifies its own write (act, re-read, diff; the API demands it, see `/emailbison-api`).

### 1. Create the campaign

```
POST /api/campaigns
{"name": "Acme US SaaS Founders", "type": "outbound"}
```

Arrives as `status: "draft"`. It stays that way until you decide otherwise.

### 2. Load ALL copy variants of each step in ONE sequence-steps POST

```
POST /api/campaigns/{id}/sequence-steps
{
  "title": "Step 1",
  "sequence_steps": [
    {"email_subject": "...", "email_body": "...", "wait_in_days": 1},
    {"email_subject": "...", "email_body": "...", "wait_in_days": 1, "variant": true},
    {"email_subject": "...", "email_body": "...", "wait_in_days": 1, "variant": true}
  ]
}
```

First item is the primary; every subsequent item carries `"variant": true` and auto-links to it. All variants in one call, never drip-fed (adding variants later has traps; editing them later means delete + re-create, see `/emailbison-api`).

Follow-up touches are separate sequential steps with real `wait_in_days` gaps, minimum 3 days between same-thread steps. The default architecture (step 2 at +3 days same thread, step 3 at +30 days new thread with a different offer, step 4 at +3 days) is in `/sequence-architecture`.

After the POST: re-read the steps and confirm the count, the variant links, and the subjects (thread-reply steps must have bare subjects, no `Re:` prefix; the API adds that itself at send time).

### 3. Settings: the deliverability set

```
PATCH /api/campaigns/{id}/update
{
  "plain_text": true,
  "open_tracking": false,
  "include_auto_replies_in_stats": false,
  "can_unsubscribe": true,
  "reputation_building": false,
  "max_emails_per_day": 1000,
  "max_new_leads_per_day": 1000
}
```

The reasoning, because you'll be tempted to fiddle:

- `plain_text: true` for outbound campaigns whose copy is plain text (which cold copy should be; no links in cold email anyway). Only a `reply_followup` campaign takes `false`.
- `open_tracking: false`, always. Tracking pixels hurt cold deliverability and open rates are vanity data.
- `include_auto_replies_in_stats: false`: out-of-office noise pollutes your reply rate, which is the number you actually steer by.
- **Daily limits at 1000+.** This looks backwards until you get it: volume is governed by your SENDERS (each connected sender's daily limit, see `/sender-scaling`), not the campaign cap. A campaign cap below sender capacity silently throttles you; one campaign sat pinned at 999/1000 for weeks while its senders could do more. Set the campaign cap out of the way and control volume where it belongs.

Then GET and diff. The PATCH endpoint resets any boolean you omit, so this exact full set gets re-sent on every future settings change too.

### 4. Upload leads: bulk upsert, then attach

Two phases, both in `${CLAUDE_SKILL_DIR}/scripts/emailbison-upload-leads.ts`:

1. **Bulk upsert** the list: `POST /api/leads/create-or-update/multiple` in batches of ~140, collecting every lead id (including existing ones matched by the upsert).
2. **Attach** to the campaign: `POST /api/campaigns/{id}/leads/attach-leads {"lead_ids": [...]}` in batches of 500.

Custom variables go on at upsert as `[{name, value}]` with UPPERCASE names (`COMPANY_NAME`, `CITY`; never `COMPANY`, it collides with the native tag). Bounced leads will refuse attach with a 422: filter them out rather than fighting the platform's memory.

Verify: campaign `total_leads` equals your list count. If it's short, find out why before moving on, don't shrug.

### 5. Attach ALL available senders

Paginate the full workspace sender list (`GET /api/sender-emails`, to `meta.last_page`, and if you get exactly 15 you've read one page), then:

```
POST /api/campaigns/{id}/attach-sender-emails
{"sender_email_ids": [every id]}
```

All of them, not some. Volume distributes across senders; more senders means fewer sends per sender, which is exactly what deliverability wants. Verify with a full match of workspace list vs attached list. A campaign with zero senders sends nothing, silently, and a fresh campaign starts at zero.

### 6. Set the schedule

```
POST /api/campaigns/{id}/schedule
{"monday": true, ..., "friday": true, "saturday": false, "sunday": false,
 "start_time": "09:00", "end_time": "17:00",
 "timezone": "America/New_York", "save_as_template": false}
```

Weekdays, business hours, the RECIPIENTS' timezone (not yours). `save_as_template: false` is required, not optional. Verify the schedule exists with the right days and timezone; ignore its `status` label, which lies.

### 7. STOP

The campaign is now complete and DRAFT. This skill's job is done.

What happens next, in order:

1. **Run `/pre-launch-qa`.** Every item, PASS/FAIL. No checklist, no launch.
2. **You press start yourself.** In the UI, or via `PATCH /api/campaigns/{id}/resume` typed by your own hand. The scripts here will not do it, and I won't add a flag for it. The moment a script can activate a campaign, someone's unreviewed campaign goes live at 9am with a broken variable in the subject line.
3. After first sends, pull `${CLAUDE_SKILL_DIR}/scripts/emailbison-pull-analytics.ts` within the hour and read the bounce replies. Over 5% bounce on the first meaningful volume: pause and run `/deliverability-triage`.

## Common gotchas

1. **One campaign per offer.** No. One campaign per LIST; offers are variants in one step.
2. **Campaign caps as volume control.** Caps at 1000+, senders govern volume. A low cap is a silent throttle.
3. **Partial settings PATCH later on.** Resets the booleans you didn't send. Full set, every time, then diff.
4. **"It's active but nothing's sending."** Zero senders attached, or you're outside the schedule window. Check `emails_sent`, not labels.
5. **Uploading variants as steps.** Incrementing `order` means every lead gets ALL of them in sequence. That's four cold emails in four days to one person. Variants carry `"variant": true`.
6. **Trusting the upload without counting.** Upsert + attach are batchy and resumable; always reconcile `total_leads` against the source list.
7. **Building on an unclean list.** Attach-time is too late to discover security-gateway domains in the set. `/list-hygiene` first.

## What to do next

**Run `/pre-launch-qa` now.** The campaign is draft and must stay draft until every check passes. After QA passes, activation is yours: one click, made by you, with the checklist in front of you.

## Related skills

- `/pre-launch-qa`: the mandatory gate between this skill and anything going live
- `/emailbison-api`: the endpoint mechanics and traps this build leans on
- `/sequence-architecture`: the step structure and wait-day rules
- `/cold-email-copywriting`: the variants that go into step 1
- `/list-hygiene`: what the list must pass before it gets uploaded
- `/sender-scaling`: why sender limits, not campaign caps, govern volume
