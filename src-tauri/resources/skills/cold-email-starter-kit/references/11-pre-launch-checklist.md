# 11: Pre-Launch Checklist

Every expensive cold email failure I've had shipped through a gap a checklist would have caught: a broken variable in a subject line, security-gateway domains in the list, settings flipped by a partial update. None were exotic. All were checkable in minutes.

The rule has no exceptions: **no checklist, no launch.** New campaign, resume after a pause, reactivation after edits, "I just added a few leads": any event that starts or restarts sending runs this gate first.

This is the condensed go/no-go version. The full gate, with the evidence each check demands, is `/pre-launch-qa`. Run that skill for real launches; use this page to know what's coming.

---

## The six check groups

### 1. Copy
- [ ] Every `{{VARIABLE}}` resolves on every attached lead (one missing value sends "Hi {{COMPANY_NAME}}" to a real person)
- [ ] Zero links, URLs, or dotted domains anywhere in cold copy
- [ ] All spintax renders; every path reads like a sentence a human typed
- [ ] Subjects lowercase, consistent across variants
- [ ] Variants are variants (`variant: true` inside one step), not extra sequential steps
- [ ] Exactly one opt-out line, bottom of every step and variant
- [ ] No stale time-bound framing ("ahead of the new year", resuming in March)

### 2. Settings
- [ ] `plain_text` matches campaign type (outbound: true; reply follow-up: always false)
- [ ] `open_tracking: false`
- [ ] Auto-replies excluded from stats
- [ ] Daily limits 1000+ (senders govern volume; a low campaign cap is a silent throttle)
- [ ] Verified by GET-and-diff, not memory (partial PATCHes reset booleans)

### 3. Leads
- [ ] Count matches: campaign `total_leads` equals the list you meant to upload
- [ ] Hygiene done: deduped, verified where needed (lists you brought; AI Ark exports ship verified)
- [ ] ZERO Mimecast or Barracuda domains, confirmed by MX-checking EVERY domain, never a sample
- [ ] No bounced-status leads attached

### 4. Senders
- [ ] ALL workspace senders attached (paginate both sides; exactly 15 means you read one page)
- [ ] All attached senders show Connected (more than ~10 disconnected: stop and investigate)
- [ ] Senders warmed 2-3 weeks, warmup still on, limits per `/sender-scaling`

### 5. Schedule
- [ ] A schedule exists (resume fails opaquely without one)
- [ ] Weekdays, business hours
- [ ] Timezone matches the RECIPIENTS' geography, not yours

### 6. Sequence
- [ ] Every gap between steps is 3+ days
- [ ] Thread structure deliberate (step 2 same thread, step 3 new thread and new offer at +30 days, per `/sequence-architecture`)
- [ ] Every variant still links to its parent step (PUT edits silently orphan them)

Every item reports PASS or FAIL with evidence. A FAIL is a stop, not a note. When everything passes, YOU press start, by hand. Nothing in this repo activates a campaign for you.

---

## Day 1: watch it like it's fragile, because it is

The first day of sending is a live test of everything above.

- **Check bounces roughly hourly for the first few hours.** Pull stats and the bounced-replies feed (`emailbison-pull-analytics.ts`) and read the actual bounce text, not just the number.
- **Over 5% bounce on the first 25+ sends: pause immediately.** Pausing is always safe and always reversible; it buys thinking time and never makes anything worse. Then run `/deliverability-triage` and diagnose before touching anything.
- **Confirm sending actually started**: `emails_sent > 0` inside the schedule window. Ignore the schedule's status label; it lies in both directions.
- End of day 1: bounce rate under 3% and volume matching your sender capacity means the machine is working. Let it run.

---

## What to expect from campaign one (honestly)

Most first campaigns are mediocre. Not because you did it wrong, but because the offer, the list, and the copy have never met a real audience before. That's what campaign one is FOR.

- A first campaign that sends cleanly (low bounces, no spam-folder disasters) is a technical success even if replies are thin.
- Judge the offer on the interested-reply rate: 20%+ of human replies is great, 10%+ is good, under 5% means the offer or list is wrong (see `09-reply-handling.md`). Most first campaigns land in the "under 5%, now iterate" zone.
- The system is iteration: read the replies, mine the objections, reframe the offer (`/offer-framing`), test the next angle. The senders and domains you built carry over; each campaign gets cheaper and better.

Anyone promising a calendar full of meetings from campaign one is selling something. What compounds is the loop.

---

## What to do next

Run `/pre-launch-qa` for the real gate, press start yourself, then do the day-1 watch above. From day 2 onward, the monitoring loop in `13-fast-track.md` takes 10 minutes a day.
