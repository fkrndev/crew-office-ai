---
name: cold-email-kickoff
description: The guided "start here" orchestrator for this whole repo. Asks a few situational questions one at a time, runs the right skills in the right order, and produces a consolidated campaign plan plus the exact next skill for your situation. Triggers on "start here", "where do I start", "I'm new", "I'm new to cold email", "set up cold email", "get me started", "kick off", "first campaign", "I just cloned this repo", "walk me through this", "onboard me", "what do I do first", "help me start", "begin", or any first invocation by someone who has not run cold email through this system before. If someone is staring at 21 skills with no idea which to run, this is the answer.
---

# Cold Email Kickoff

The single entry point. Twenty skills is a library, not a plan. This skill turns it into a plan by asking where you actually are, then running the right chain and handing you off to the exact next step.

I built this because every new operator asks the same question ("where do I start?") and the answer depends on four facts about their situation. So ask for the four facts, then stop guessing.

## The rules for running this skill

These are not suggestions:

1. **One question at a time.** Ask a question, wait for the answer, then ask the next. Never dump all four questions in one message. A wall of questions gets a wall of half-answers.
2. **Play back what you heard** before moving on. "So: no inboxes yet, rough idea of who to target, never sent cold email, goal is booked sales calls. Right?" Cheap to ask, expensive to skip.
3. **Don't duplicate other skills.** This skill decides WHICH skills run and in what order. It runs them by name (`/icp-builder`, `/offer-framing`, and so on). It never re-implements their content inline.
4. **Skip what's already done.** If `profiles/profile.yaml` exists, ask whether to reuse it before interviewing anyone. If they have warmed inboxes, don't lecture them about warmup.

## Step 1: Four questions, one at a time

Ask these in order. Each answer changes what happens next.

**Q1: "Do you already have sending infrastructure? Meaning: separate sending domains with inboxes that have been warming for at least 2 to 3 weeks, connected to EmailBison or ready to be."**

This goes first because it's the longest lead time in the whole system. If the answer is no, everything else happens during the warmup window, not before it. Record the answer as yes / no / not sure ("not sure" is a no).

**Q2: "Do you have a clear picture of who you're targeting? Industries, job titles, company size, geography. Or is it still fuzzy?"**

Clear means they can rattle it off. Fuzzy means "founders, probably" or "anyone who'd buy". Be honest with yourself here: fuzzy targeting with sharp copy still loses.

**Q3: "Have you run cold email before? At real volume, not five emails from your main inbox."**

This calibrates depth, not the route. Experienced operators get the fast path through each skill (confirm and move). New operators get the full interviews and the reasoning behind each decision.

**Q4: "What's the goal? Booked calls, replies to a specific offer, testing a new market, filling a pipeline for a launch, something else?"**

The goal shapes the offer work. "Booked calls for a service" and "validate a new product idea" produce different roadmaps.

## Step 2: Play it back

Summarize the four answers in two or three sentences and get a "yes, that's right" before running anything. If they correct you, update and play it back again.

## What the flow looks like in practice

The shape of a good kickoff conversation:

> **You:** Before I point you anywhere: do you already have sending infrastructure? Separate domains with inboxes that have been warming 2 to 3 weeks?
>
> **Them:** No, I was going to send from my normal work email.
>
> **You:** Glad we caught that now. Sending cold volume from your main domain is how you burn the domain your actual business runs on. We'll set up separate infrastructure, and the warmup wait becomes the window where we build everything else. Next: do you have a clear picture of who you're targeting? Industries, titles, company size, geography?
>
> **Them:** Roughly. B2B SaaS founders, I think.
>
> **You:** That's a start, and "I think" tells me it's worth sharpening. Two more questions, then I'll play the whole thing back.

Then Q3, then Q4, then the playback, then the chain. Notice what did NOT happen: no four-question dump, no lecture, no skipping ahead to copy. Each answer got a one-line reaction so they know it landed, and the fuzzy ICP (Ideal Customer Profile: who exactly you're targeting, by industry, title, size, geography) got flagged without being solved on the spot. That's `/icp-builder`'s job.

## Step 3: Run the strategy chain

Everyone goes through strategy before touching lists, copy, or sending. This is deliberate: the offer decides whether the campaign works, and the offer comes from the ICP. Run these in order:

1. **`/icp-builder`**. Produces `profiles/profile.yaml`, the file every other skill reads.
   - Profile already exists: ask "reuse or rebuild?" and respect the answer.
   - Q2 was "clear": run the fast path. Capture what they told you into the profile, confirm, move on. Don't re-interview someone who already knows their market.
   - Q2 was "fuzzy": run the full interview. This is where fuzzy becomes clear.
2. **`/offer-framing`**. Decides what the emails actually offer. Front-end the service first; fall back to a lead magnet if there's no service offer strong enough to lead with. Most campaigns fail here, not in the copy.
3. **`/lead-magnet-builder`**, only if the framing landed on the lead magnet path or an angle needs a real free asset behind it. If the service offer stands on its own, skip this. Don't build assets nobody asked for.
4. **`/offer-testing-roadmap`**. Turns the offer into a week-by-week roadmap of angles. One offer is a bet; a roadmap is a system.

Each skill hands its output to the next through `profiles/profile.yaml` and its own artifacts. Let them do their jobs; don't summarize their content here.

## Step 4: Write the campaign plan

Consolidate everything into `profiles/campaign-plan.md`. This is the one-page version someone could act on without reading anything else. Structure:

```markdown
# Campaign Plan
Generated: YYYY-MM-DD

## ICP summary
- Industries:
- Titles:
- Company size:
- Geography:
- Hard exclusions:
(pulled from profiles/profile.yaml)

## Offer
- Chosen offer:
- Frame: Offer First / Built For You
- Why this offer: (one line, from /offer-framing)
- Free asset behind it: (name it, or "none needed")

## First 4 angles to test
(from /offer-testing-roadmap, one per week)
1.
2.
3.
4.

## Infrastructure status
- Sending domains + inboxes: ready / warming until DATE / not started
- List: built / not built
- Copy: written / not written

## Next actions
1. /skill-name: what it does for you
2. /skill-name: what it does for you
3. /pre-launch-qa before anything goes live. Always.
```

Fill "Next actions" from the branch below. `profiles/` is gitignored, so real business data stays out of the repo.

## Step 5: The handoff menu

Branch on where they actually are. Present the matching option first, but show the map so they can see the whole road:

**No infrastructure (Q1 = no):**
Next is `/domain-inbox-setup`. It spends real money on domains and inboxes, so it confirms before every purchase. Warmup takes 2 to 3 weeks, and that window is exactly enough time to build the list (`/ai-ark-list-builder`) and write the copy (`/cold-email-copywriting`). Nobody sits idle during warmup.

**Infrastructure ready, no list:**
Next is `/ai-ark-list-builder`. The profile written in Step 3 feeds the targeting directly. After the export, `/list-hygiene` cleans it before it touches anything.

**List ready, no copy or campaign:**
Next is `/cold-email-copywriting` for the week 1 angle from the roadmap, then `/emailbison-campaign-launch` to assemble the campaign as a DRAFT. It never activates anything; you press start yourself.

**Everything ready (infra + list + copy + campaign built):**
Next is `/pre-launch-qa`. No checklist, no launch. When every item passes, activation is one click, made by you.

End by printing the path to `profiles/campaign-plan.md` and naming the single next skill to invoke.

## Safeguards

- **Never overwrite an existing `profiles/profile.yaml` without asking.** Someone's real business context lives in that file.
- **Never auto-invoke `/domain-inbox-setup` purchases.** It spends money. Offer it, let them start it.
- **Never skip the warmup reality.** If someone with fresh inboxes wants to send today, say no and say why: cold inboxes get campaigns buried in spam before they start.
- **Never let anything activate.** Campaigns land as drafts. The gate is `/pre-launch-qa`, and the start button belongs to the user.

## What to do next

Follow the branch from Step 5. That's the whole point of this skill: it ends with exactly one next command for your situation, not a menu of twenty.

If you stopped mid-way (waiting on warmup, for example), come back and re-run `/cold-email-kickoff`. It finds your existing profile and plan, skips what's done, and picks up at the right step.

If you'd rather learn every piece deeply before moving fast, `/cold-email-starter-kit` is the full tutorial version of this same road.

## Related skills

- `/icp-builder`: run in Step 3, writes `profiles/profile.yaml`
- `/offer-framing`: run in Step 3, decides what the emails offer
- `/lead-magnet-builder`: run in Step 3 only when the frame needs a free asset
- `/offer-testing-roadmap`: run in Step 3, the week-by-week angle plan
- `/domain-inbox-setup`: handoff when there's no sending infrastructure
- `/ai-ark-list-builder`: handoff when infra is ready but there's no list
- `/cold-email-copywriting` and `/emailbison-campaign-launch`: handoff when the list is ready
- `/pre-launch-qa`: the gate before anything ever goes live
- `docs/roadmap.md`: the full "which skill do I need right now" map
