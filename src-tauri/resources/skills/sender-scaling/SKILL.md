---
name: sender-scaling
description: "The volume discipline for cold email senders: every new sender starts at 1 real email per day, scales to 3 per day only on proven engagement (a strong reply rate with actual interested replies), and total volume grows by adding MORE senders, never by cranking per-sender limits. Warmup stays on forever. Triggers on 'increase volume', 'send more emails', 'scale up', 'raise the daily limit', 'how many emails per day', 'how many senders do I need', 'ramp up sending', 'why is volume so low', 'can I send 50 per inbox'."
---

# Sender Scaling: 1 a day until proven, then more senders

The fastest way to kill cold email infrastructure is impatience. A new sender blasting 30 emails a day into an unproven campaign looks exactly like a spammer to every receiving server, because that is exactly what a spammer looks like: new domain, high volume, no engagement history. The domains get throttled, then blocked, and you're back at `/domain-inbox-setup` buying replacements three weeks behind schedule.

The discipline is simple and it is not negotiable:

1. **Every new sender starts at 1 real email per day.**
2. **Scaling to 3 per day is earned by engagement, not by time passing.**
3. **Volume comes from MORE senders, never from higher per-sender limits.**
4. **Warmup stays on. Forever.**

## When to use this

- Setting daily limits on freshly warmed senders
- "Can we send more?" in any form
- Deciding how many domains and inboxes to buy for a target volume
- Diagnosing why total volume is lower than expected
- Reviewing sender settings during `/pre-launch-qa`

## The scaling ladder

### Stage 0: warmup only (weeks 1-3 of a sender's life)

Fresh inboxes from `/domain-inbox-setup` send nothing but warmup traffic for 2-3 weeks. No real sends. None.

### Stage 1: 1 real email per day per sender

When warmup completes, the sender enters real sending at **1 per day**, warmup still running underneath. This is the state every sender lives in until the CAMPAIGN it serves proves itself.

This feels absurdly slow until you do the maths on a proper fleet: 20 domains with 2 inboxes each is 40 senders, which is 40 real sends a day at stage 1, on top of warmup traffic keeping every domain's reputation moving. That's meaningful test volume with essentially zero reputation risk on any single domain.

### Stage 2: 3 per day, unlocked by proven engagement

The unlock is **a strong interested-reply rate, not a calendar date**. My benchmarks on interested replies (as a share of all replies): 20%+ is great, 10%+ is good, under 5% is bad. When a campaign is pulling a strong reply rate with genuine interested replies in it, the offer and list have proven themselves, and the senders serving that campaign can go to **3 real emails per day**.

Positive engagement is itself a reputation signal: receiving servers see replies to your mail and trust the domain more. That's why proof comes first; scaling volume on a campaign nobody answers just accelerates the damage.

If the campaign hasn't proven itself, the fix is never "more volume". It's a better offer, a better list, or better copy (`/offer-framing`, `/list-hygiene`, `/cold-email-copywriting`). Volume is fuel; it multiplies whatever is already happening, including nothing.

### Stage 3: more volume = more senders

Past 3 per day per sender, you do not raise limits. You add senders. Want to go from 120 to 240 sends a day? That's 40 more proven-stage senders (20 more domains via `/domain-inbox-setup`), not 40 senders doing 6 each.

Why this is the hill I'll die on: per-sender volume is the primary spam-filter heuristic that you control. Reputation damage is also per-domain, so a fleet of low-volume senders diversifies the risk; one throttled domain in a fleet of 40 costs you 2.5% of capacity, not everything. Cheap domains and inboxes made this trade one-sided: sender capacity is nearly free, reputation is not.

## The planning maths

Work backwards from target volume, at stage-2 rates and 2 inboxes per domain:

| Target sends/day | Senders needed | Domains needed |
|---|---|---|
| 30 | 10 | 5 |
| 120 | 40 | 20 |
| 300 | 100 | 50 |
| 600 | 200 | 100 |

Buy domains for next month's volume, not today's, because every new domain sits in warmup for 2-3 weeks before it contributes anything.

One mechanical note for EmailBison: per-sender daily limits are where volume is actually governed. Campaign-level caps get set high (1000+) precisely so they never interfere; a campaign cap below sender capacity is a silent throttle you'll forget about (see `/emailbison-campaign-launch`).

## Common gotchas

1. **Scaling on time instead of proof.** "It's been two weeks, let's go to 10 a day." No. The unlock is interested replies, and the step is 3, not 10.
2. **Turning warmup off once real sending starts.** Warmup is permanent background reputation maintenance, not a launch phase. Leave it on.
3. **Raising per-sender limits because buying domains feels slow.** Three weeks of warmup beats a burned fleet. Buy ahead instead.
4. **Scaling volume on a campaign with a bad reply rate.** Under 5% interested with real volume behind it means the offer or list is wrong. More sends make it worse, and now with reputation damage attached.
5. **Reading total volume as the health metric.** The health metrics are per-sender volume, bounce rate, and interested-reply rate. Total volume is just the multiplication.
6. **Uneven distribution.** Attach ALL senders to every campaign so the load spreads; a subset of senders carrying the whole campaign defeats the entire model.

## What to do next

If your senders are new, park them at 1/day and go improve what actually unlocks scale: `/offer-framing` and `/cold-email-copywriting`. If your campaign has proven engagement, move its senders to 3/day and start `/domain-inbox-setup` for the next batch of domains, because they'll take 2-3 weeks to warm. Before any limit change goes live with a resume, run `/pre-launch-qa`.

## Related skills

- `/domain-inbox-setup`: where new senders come from, and the warmup that gates them
- `/emailbison-campaign-launch`: why campaign caps sit high while senders govern volume
- `/pre-launch-qa`: checks sender limits match this ladder before launch
- `/deliverability-triage`: what to run when scaling went wrong anyway
- `/reply-classification`: how interested replies (the unlock metric) are counted
