---
name: cold-email-starter-kit
description: The end-to-end cold email tutorial, from nothing to a launched campaign on my stack (AI Ark lists, Dynadot domains, Zapmail inboxes, EmailBison sending, Kitt verification for outside lists). Use when starting cold email from scratch, learning the whole system step by step, teaching someone cold email, setting up a first campaign, or when you want the full manual instead of a guided flow. Triggers on "how do I start cold email", "cold email from zero", "set up cold email", "first campaign", "cold email tutorial", "walk me through cold email", "where do I start", "cold email for beginners", "end to end cold email", "the starter kit", "teach me cold email", "cold email 101".
---

# Cold Email Starter Kit

This is the full manual. Follow it top to bottom and you will go from nothing to a launched campaign: warmed sending infrastructure, a clean targeted lead list, an offer worth replying to, a 4-step sequence, and a draft campaign in EmailBison that YOU press start on after QA.

It is built from the system that runs every client account at my agency, sanitized so a stranger with fresh accounts can run it. No prior cold email experience required. Terminal comfort required.

## What you'll build

By the end of the flow:

- 10-20 lookalike sending domains bought on Dynadot
- 2 inboxes per domain on Zapmail, warmed for 2-3 weeks, SPF/DKIM/DMARC handled
- A defined ICP and a framed offer with a testing roadmap behind it
- A targeted lead list from AI Ark, deduped, exclusion-checked, MX-checked
- A 4-step sequence in the house copy style
- A DRAFT campaign in EmailBison, QA-passed, ready for you to activate

Realistic cost (all approximate, check current pricing yourself):

| Item | Approx cost | Notes |
|---|---|---|
| 20 x .com domains on Dynadot | ~$12 each, ~$240/yr | Paid annually. Start with 10 if budget is tight. |
| Zapmail, 40 inboxes | ~$60/mo | 2 inboxes per domain across 20 domains. |
| AI Ark export credits | free tier: 100/mo. Paid from $49/mo for 5,000 | Previews are free. Exports cost about 1 credit per lead. |
| EmailBison subscription | $599/mo flat, 500k emails included | Invite-only, no free trial. Apply early, approval takes time. |
| Kitt verification | optional | Only for lists you bring from elsewhere. AI Ark exports never need it. |

Budget roughly $300-500 for month one on domains, inboxes, and list credits, plus the $599/mo EmailBison subscription once you're past testing. EmailBison and the domain/inbox setup both have lead time (approval, warmup), so start both on day one regardless of where you are on the rest.

## The end-to-end flow

1. **Deliverability basics**, read once so the rest makes sense (`references/01`)
2. **Infrastructure**, buy domains and provision inboxes NOW because warmup takes 2-3 weeks and everything else happens inside that window (`references/05`)
3. **Offer and strategy**, decide what the emails actually offer (`references/02`)
4. **Copywriting**, write the variants (`references/03`)
5. **Sequence**, arrange them into the 4-step structure (`references/04`)
6. **List building**, AI Ark export plus hygiene (`references/06`)
7. **Launch**, assemble the draft campaign, run `/pre-launch-qa`, then you press start
8. **Replies and iteration**, classify what comes back, follow up, test the next angle

Step 2 comes early on purpose. The warmup clock is the longest pole in the tent; start it first and be launch-ready the day it finishes.

## Prerequisites

**Technical:** Node.js 20+ (`node --version`), a terminal you're comfortable in, any text editor. The repo ships `tsx` as a dev dependency, so `npm install` at the repo root covers the runtime.

**Accounts:** AI Ark, EmailBison, Dynadot, Zapmail. Kitt only if you'll import lists from outside AI Ark. Signup details and key locations are in `references/00-getting-started.md` and in `.env.example`.

**Budget:** see the table above. If ~$300 is a stretch, halve the domain count and go slower rather than skipping warmup or hygiene.

## Escape hatches (you may not need this manual)

- **Most people should start with `/cold-email-kickoff`** instead. It runs ICP, offer, strategy, and plan as one guided flow and hands you to the right next skill. This starter kit is the deeper manual for understanding each piece.
- **Experienced and just switching stacks?** Jump straight to the skill you need: `/ai-ark-list-builder` for lists, `/domain-inbox-setup` for infrastructure, `/emailbison-campaign-launch` for the build. Every skill folder stands alone.
- **Something specific is broken?** `/deliverability-triage` for bounces and spam placement, `/offer-framing` for flat reply rates.

## Reading order

The `references/` folder is numbered in first-read order:

| # | File | What it covers |
|---|---|---|
| 00 | `00-getting-started.md` | Screener, accounts, costs, `.env` setup, what the first 30 days look like |
| 01 | `01-deliverability-basics.md` | Separate domains, SPF/DKIM/DMARC, warmup, volume math, burned reputation |
| 02 | `02-offers-and-strategy.md` | The offer doctrine condensed: diagnosis order, free value, the two frames |
| 03 | `03-copywriting.md` | House copy rules condensed, plus one complete worked example |
| 04 | `04-sequence-structure.md` | The 4-step architecture with a full copy-pasteable sequence |
| 05 | `05-domain-inbox-setup.md` | Dynadot, Zapmail, EmailBison walkthrough with the script commands |
| 06 | `06-list-building.md` | AI Ark end to end, the verification rule, the MX check |
| 07 | `07-verification.md` | When to verify (your own lists, with Kitt) and when never to (AI Ark exports) |
| 08 | `08-emailbison.md` | EmailBison API reference: pagination, the PATCH trap, variables, the traps table |
| 09 | `09-reply-handling.md` | Classification, drafting, and follow-ups, condensed |
| 10 | `10-legal-compliance.md` | CAN-SPAM, GDPR, unsubscribes, physical address, in plain English |
| 11 | `11-pre-launch-checklist.md` | The QA gate before anything goes live |
| 12 | `12-troubleshooting.md` | Symptom to cause to fix |
| 13 | `13-fast-track.md` | The timed "launch this week" walkthrough |

## Decision tree: where am I, what do I open?

| Your situation | Go to |
|---|---|
| "I've never sent a cold email" | `references/00`, then `references/01`, then follow the flow in order |
| "I have no domains or inboxes" | `references/05-domain-inbox-setup.md`, or `/domain-inbox-setup` |
| "I don't know what to offer" | `references/02`, then `/offer-framing` |
| "I need to write the emails" | `references/03` + `references/04`, then `/cold-email-copywriting` |
| "I need a lead list" | `references/06`, or `/ai-ark-list-builder` |
| "I have a list from somewhere else" | `/email-verification`, then `/list-hygiene` |
| "Ready to build the campaign" | `/emailbison-campaign-launch` |
| "About to press start" | `/pre-launch-qa`. Always. No checklist, no launch. |
| "Replies are coming in" | `/reply-classification`, then `/reply-drafting` |
| "Bounces are high or nothing lands" | `/deliverability-triage` |
| "Replies are flat and I'm tempted to send more" | `references/02`. Volume is fuel, not the fix. |

## Joel-proven defaults

Use these unless you have a measured reason to deviate. They are what every campaign I run starts on:

- **20 domains x 2 inboxes = 40 senders.** Lookalike .com domains, .co fallback. Never your real domain.
- **Every new sender starts at 1 real email/day.** Scale to 3/day only on proven engagement (a strong reply rate with genuine interested replies). More volume means more senders, never higher per-sender limits.
- **Warmup 2-3 weeks before real sends, and it stays on forever.**
- **4-step sequence:** day 0, +3 days same thread, +30 days NEW thread with a NEW offer angle, +3 days same thread. Minimum 3 days between any two steps, always.
- **50-90 words per email.** One sentence per paragraph.
- **Subject lines lowercase, 2-4 words.** Threaded steps get no subject at all.
- **ONE campaign per lead list.** All copy variants go into a single step as A/B variants. Cap around 6,000 leads per campaign; segment bigger lists.
- **Open tracking off.** Pixels hurt deliverability and opens are vanity data.
- **Plain text on for outbound campaigns.** The HTML exception exists only for reply follow-ups with a clickable calendar link, which is a different system.
- **Judge on interested reply rate:** 20%+ of replies interested is great, 10%+ is good, under 5% means the offer or list is wrong.

## Ten common beginner mistakes

1. **Sending from your main domain.** One spam stain and your invoices land in junk. Lookalike throwaways only.
2. **Skipping or shortening warmup.** An unwarmed inbox sends fine and delivers nowhere. 2-3 weeks, no exceptions.
3. **Cranking per-sender limits to get volume.** High per-sender volume is THE spam heuristic. Add senders instead.
4. **Links, email addresses, or domains in cold copy.** The single biggest spam trigger in a first touch. None, anywhere, including the signature.
5. **Mentioning calls or time in the first email.** Cold email sells the reply, nothing else. Time asks come later, in the reply thread.
6. **Re-verifying AI Ark exports.** They're verified at source. Paying to verify them again buys you nothing.
7. **Skipping the MX check.** Mimecast and Barracuda protected domains reject cold mail on policy and wreck your bounce rate. Silent, automatic exclusion, every list.
8. **Judging a list by page 1.** AI Ark results cluster the worst matches at the back. Check the last pages before every export.
9. **Testing five wordings of one offer.** Early on you test ANGLES, not phrasings. The offer is the big lever; copy sharpens a winner, it cannot create one.
10. **Launching without the QA gate.** `/pre-launch-qa` before any launch, resume, or reactivation. The one time you skip it is the time the variables don't resolve.

## Scripts

Everything runs with `npx tsx <path>` after `npm install` at the repo root. Keys come from `.env` only, never flags. Anything that spends money prints the bill and waits for your explicit yes.

| Script | Purpose |
|---|---|
| `${CLAUDE_SKILL_DIR}/scripts/_lib.ts` | Shared zero-dependency runtime (env, CSV, retry, confirm prompts). Not run directly. |
| `${CLAUDE_SKILL_DIR}/scripts/setup-env.sh` | Creates your `.env` from the template, never overwrites an existing one |
| `${CLAUDE_SKILL_DIR}/scripts/verify-credentials.ts` | Pings every configured API, prints a pass/warn/fail table (`npm run verify`) |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts` | AI Ark preview, confirm, export, poll, fetch to CSV |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/email-verification/scripts/kitt-verify.ts` | Kitt verification for lists you bring from elsewhere, resumable, capped |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/list-hygiene/scripts/mx-exclusion-check.ts` | Drops Mimecast/Barracuda protected domains before any send |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-generate-domains.ts` | Generate lookalike domain candidates |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-bulk-purchase.ts` | Availability check and purchase, confirms total cost first |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/zapmail-setup.ts` | Connect domains, create inboxes, poll to ACTIVE |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/emailbison-campaign-launch/scripts/emailbison-create-campaign.ts` | Create the draft campaign, steps, senders, schedule |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/emailbison-campaign-launch/scripts/emailbison-upload-leads.ts` | Batch lead upload and attach |
| `${CLAUDE_PROJECT_DIR}/.claude/skills/emailbison-campaign-launch/scripts/emailbison-pull-analytics.ts` | Pull campaign stats to CSV |

## A note on ethics

Cold email is legal when done right and only stays effective if you treat inboxes with respect. B2B only, never consumers. Honor opt-outs instantly and forever. Real physical address in the footer. Send things you'd be glad to receive. The whole system in this repo is built on giving away genuine value first; if your plan is to extract rather than give, this kit will not save you.

## What to do next

If you haven't already, run `/cold-email-kickoff` for the guided version. Otherwise open `references/00-getting-started.md` and start the flow. The references chain into each other, and each hands off to the full skill when you're ready to execute.

## Related skills

- `/cold-email-kickoff`: the guided entry point most people should use first
- `/icp-builder`: produces the `profile.yaml` the whole system reads
- `/domain-inbox-setup`, `/ai-ark-list-builder`, `/cold-email-copywriting`, `/sequence-architecture`, `/emailbison-campaign-launch`: the execution skills this kit teaches the theory behind
- `/pre-launch-qa`: the mandatory gate before anything goes live
