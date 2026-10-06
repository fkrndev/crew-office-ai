# 13: The Fast Track

The timed walkthrough for someone who wants to move NOW. It assumes you've signed up for the accounts (doc `00-getting-started.md`) and have your API keys ready to paste.

One honest thing before the clock starts: **the active work in this plan fits inside a week. The calendar time doesn't, because warmup takes 2-3 weeks and there is no way around it.** Anyone who tells you to skip warmup is volunteering your domains for the spam folder. If you already have warmed inboxes from an existing setup, you genuinely can launch this week: do Day 1 and Day 2, skip the wait, go straight to launch day.

Total active time: roughly 3 hours across the whole plan. Total calendar time from zero: about three weeks, most of it waiting while machines do reputation work you can't accelerate.

---

## Day 1: infrastructure kicked off and targeting drafted (60-90 minutes active)

### Env and verify (10 min)

```bash
cd cold-email-playbooks
cp .env.example .env
# paste your keys into .env
npm run verify
```

Every service you plan to use shows ✅ before you continue. Anything ❌ gets fixed now, not later: `12-troubleshooting.md` has the per-service fixes and auth one-liners. Ten minutes here saves a day of confusion downstream.

### Buy domains (20-30 min)

Full detail in `05-domain-inbox-setup.md` and `/domain-inbox-setup`. The compressed version:

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-generate-domains.ts   # candidates from your brand
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-bulk-purchase.ts      # checks availability, prints the bill
```

The purchase script prints the exact total cost and stops for your explicit yes before buying anything. Read the number, then decide. For a first run, 5-10 domains is plenty: with 2 inboxes each and safe sending limits, 10 domains supports roughly 60 sends/day once proven. Never send cold email from your main domain, that's the whole point of these.

### Kick off Zapmail (10 min active, then walk away)

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/zapmail-setup.ts
```

This switches nameservers, waits out DNS propagation, connects the domains to Zapmail, polls until they're assignable, and creates 2 inboxes per domain. The active part is starting it and answering its prompts; the rest is polling and waiting. Inboxes show "In Progress" for 4-6 hours after creation. Leave them alone.

### Targeting draft and preview (20-30 min)

While the machines work, define who you're emailing. Read `06-list-building.md` if you haven't, then create `targeting.json` (the shape is documented in the script header) and preview:

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts --preview --config targeting.json
```

Previews are essentially free; exports cost about 1 credit per record, which is why we preview today and export tomorrow. Check the count against your volume target, then check the LAST pages of the results, not the first: AI Ark orders best-first, so the back pages are the honest ones. Off-ICP junk in the back pages means tighten the excludes and preview again. Loop until the back page is clean.

Day 1 done. Active time spent: about an hour and a half. You now have domains propagating, inboxes provisioning, and targeting you trust.

---

## Days 1-2: the passive stretch

DNS propagates, Zapmail provisions the inboxes (4-6 hours), and once they're ACTIVE they enter warmup. Nothing here needs you. Check in once on Day 2 to confirm every inbox reads ACTIVE and warmup is running; retry any stragglers individually.

---

## Day 2: the list (30 minutes active)

### Export the leads

Targeting was proven yesterday, so today is mechanical:

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts --submit --config targeting.json
```

The script prints the record count and estimated credit cost and waits for your confirmation before spending. Exports are async: it submits, polls the tracking ID (a 409 while polling just means still processing), and writes the CSV when done.

### Hygiene, and verification only if you brought your own list

- **AI Ark export:** do NOT verify it. It's verified at source; paying to verify it again is the classic beginner tax (doc `07-verification.md`).
- **A list from anywhere else** (scraped, bought, CRM export): verify it now with `kitt-verify.ts`, keep `valid`, decide on `valid-risky`, drop `invalid`.
- **Every list, no exceptions:** run `/list-hygiene`. Dedup, exclusion lists, and the MX check that automatically drops Mimecast and Barracuda domains. Those gateways reject new-domain cold mail on policy, not content, so there's no judgment call here: exclude them, always. This step is not optional.

You now have a clean `leads.csv` sitting ready, roughly 19 days before anything can send to it. Good. That order of operations is deliberate.

---

## Days 3-21: the warmup wait (and what to do with it)

Your inboxes are warming. 2-3 weeks, non-negotiable, and warmup stays on even after real sending starts. An under-warmed inbox lands in spam, stains the domain, and no copy fixes that.

Here's the reframe: this wait is a gift. The most common cold email failure isn't technical, it's launching with an offer nobody wants because all the energy went into infrastructure. You have three weeks to make sure that isn't you. Use them:

- **Week 1: the offer.** Run `/offer-framing` properly, not in ten minutes. What you're actually offering, framed by distance from the point of sale, is the single biggest lever on results. Then `/offer-testing-roadmap` so campaign two and three are already planned.
- **Week 2: the copy.** `/cold-email-copywriting` for the variants, `/sequence-architecture` for the 4-step structure, `/spintax-depth` if your volume warrants it. Write more variants than you think you need; testing is the system.
- **Week 3: the reply machine.** Read `09-reply-handling.md` and draft your reply shapes and all 8 follow-up touches in advance. The prospect who replies on launch day should get an answer in minutes, not while you scramble to write one.

Done early? Preview a second audience segment for campaign two. Do not spend the spare time inventing reasons to shorten warmup.

---

## Launch day (Day 21, or whenever warmup genuinely finishes): 45-60 minutes

1. **Build the campaign** with `/emailbison-campaign-launch`: create as draft, load every variant into one step (one campaign per list, offers as variants), settings, upload and attach the leads, attach ALL senders, set the schedule in the recipients' timezone.
2. **Run `/pre-launch-qa`.** Every item, PASS or FAIL with evidence. A FAIL is a stop, not a note. The checklist takes ten minutes; the failures it catches cost weeks.
3. **Press start yourself.** Nothing in this repo activates a campaign, ever, and that's deliberate. When every item passes, you activate it by your own hand, in the UI or via the resume call.
4. **Watch the first hour.** Pull analytics, read the bounced replies. Over 5% bounce on the first 25+ sends: pause (always safe, always reversible) and run `/deliverability-triage` before anything else.

---

## After launch: the daily 10-minute loop

Every sending day, same order:

1. **Pull the numbers** (2 min): `emailbison-pull-analytics.ts`. Sent today, bounces, replies.
2. **Check bounces against the thresholds** (1 min): sustained above 6% across 2+ days means `/deliverability-triage`. A single-day spike on small volume is noise; never act on one bad day.
3. **Classify every new reply** (3 min): buckets per `09-reply-handling.md`, strictly. Unsubscribes suppressed immediately.
4. **Answer interested replies and objections** (rest of the loop, and the only part allowed to run long): deliver the promise, two exact slots, clickable link. This is the highest-value ten minutes in the whole system; a fast reply is worth more than any copy tweak.
5. **Once a week, not daily:** compute the interested-reply rate and judge it honestly. 20%+ great, 10%+ good, under 5% means the offer or list needs work before more volume does.

That's the machine. Campaign one teaches you, campaign two uses what it taught, and the infrastructure you built this week carries all of them.

---

## The plan on one line each

| When | What | Active time |
|---|---|---|
| Day 1 | Env + verify, buy domains (confirm the bill), kick off Zapmail, targeting.json + preview | 60-90 min |
| Days 1-2 | DNS propagates, inboxes provision, warmup begins | ~0 |
| Day 2 | Export leads (confirm the cost), verify only if it's your own list, hygiene | 30 min |
| Days 3-21 | Warmup. Offer, copy, sequence, reply prep | your call |
| Day 21 | Build draft, `/pre-launch-qa`, press start yourself, watch hour one | 45-60 min |
| Daily after | The 10-minute monitoring loop | 10 min |

If any step fights you, `12-troubleshooting.md` first, then the numbered doc for that step.
