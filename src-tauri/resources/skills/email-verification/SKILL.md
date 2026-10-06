---
name: email-verification
description: Verify email lists with Kitt before sending, and know when NOT to verify at all. Triggers on "verify this list", "verify emails", "are these emails valid", "clean this list", "check deliverability of my list", "catch-all emails", "should I verify", "bought a list", "scraped a list", "CRM export", "Kitt". The rule that saves you money: AI Ark exports are verified at source and must never be re-verified; Kitt is strictly for lists you bring from elsewhere (scraped, purchased, CRM exports, old lists). Covers running the verifier, reading the valid / valid-risky / invalid verdicts, and the 402 trap.
---

# Email Verification (Kitt)

Bad addresses bounce, bounces torch your sending domains, and dead domains kill campaigns that would otherwise have worked. So yes, verification matters. But the most common verification mistake I see isn't skipping it. It's paying for it twice.

## Rule #1: never re-verify an AI Ark export

**AI Ark exports are verified at source. Do NOT run them through a verifier.** Spending money re-verifying an already-verified list is a beginner mistake, it adds cost and time and changes nothing. If your list came out of `/ai-ark-list-builder`, skip this skill entirely and go straight to `/list-hygiene`.

This rule is absolute in this system. When someone tells you "always verify everything", they're selling verification credits.

One clarification so you don't over-apply the rule: the MX gateway check in `/list-hygiene` is NOT verification. Verification asks "is this address real". The MX check asks "will this gateway accept our mail at all". Different questions, different answers. AI Ark lists skip verification, they never skip the MX check.

## When Kitt IS the right tool

Verify every list you bring from anywhere else, no exceptions:

- **Scraped lists** (any source)
- **Purchased lists** (especially these, they're the stalest)
- **CRM exports** (people change jobs constantly; a two-year-old CRM contact is a coin flip)
- **Old campaign lists** you're recycling
- **Client-supplied or partner-supplied CSVs**

Unverified outside lists routinely run 10-30% invalid. Sending to that burns your domains for weeks. The verification spend is a rounding error next to the deliverability damage it prevents.

## How to run it

The reference script wraps the Kitt API with the safety behaviour built in:

```bash
npx tsx ${CLAUDE_SKILL_DIR}/scripts/kitt-verify.ts --csv leads.csv --cap 500
```

What it does, and why each part matters:

- **Concurrency 3, maximum.** The realtime API throttles hard above that. Cranking concurrency does not make the run faster, it makes it fail.
- **Resumable.** State is written to disk after every chunk. If the run dies (network, credits, Ctrl-C), re-running skips everything already verified. You never pay twice for the same address.
- **Capped per run.** The script caps how many addresses one invocation will verify, and tells you the count and estimated cost before starting. Confirm before it spends. On a big or expensive list, run a small batch first (a few hundred), sanity-check the verdict distribution, then let the full run go.
- **Retries with backoff.** Transient failures retry automatically; you don't babysit it.

Flags: `--csv <file>` (required), `--cap N` (default 500, controls spend), `--email-column name` (default "email"). Output is always `verified-leads.csv` plus a resumable `.kitt-state.json`.

## Reading the verdicts

Every address comes back with one of three verdicts. Here's what to actually do with each:

| Verdict | Meaning | What I do |
|---|---|---|
| `valid` | Mailbox confirmed | Send. This is your core list. |
| `valid-risky` | Catch-all domain, the server accepts everything so the individual mailbox can't be confirmed | Decide per campaign, see below |
| `invalid` | Mailbox confirmed dead | Drop, always. No exceptions, no "but it's the perfect prospect". |

**The valid-risky call** is a genuine judgment, not a rule:

- Fresh sending infrastructure, or domains you're still warming: **exclude them.** You have no bounce budget to gamble with.
- Established infrastructure, healthy bounce rates, and a list that's thin without them: **include them**, but watch bounces daily and pull the segment the moment they trend toward the 6% danger line.
- A useful middle path: send to valid first, and hold valid-risky as a second wave once the campaign has proven itself.

Also look at the valid-risky share as a signal about the SOURCE, not just the addresses. A normal B2B list runs maybe 10-25% catch-all. A list that comes back 50%+ valid-risky is telling you the source was low quality, treat everything else about it with suspicion too. More on that in `/list-hygiene`.

## The 402 trap

Kitt returns **402 for two completely different reasons: you're being throttled, OR you're out of credits.** Naive scripts treat every 402 as fatal (and stop mid-list) or as retryable (and hammer a dead account forever). Both are wrong.

The script disambiguates by reading the response body: if it mentions funds, credits, or balance, you're out of money and it stops gracefully with state saved, top up and re-run to resume. Otherwise it's throttling and it backs off and retries. One more wrinkle: credit exhaustion sometimes arrives as a raw 500 instead, so the script checks every non-2xx body for the funds keywords before retrying. If you write your own client, copy this behaviour exactly.

## Common gotchas

- **Re-verifying AI Ark exports.** Rule #1. It's the whole reason this skill exists.
- **Verifying before deciding the list is worth it.** On a recycled or bought list, sample it first: check geography, check how many domains are obviously off-ICP. A big raw number is not a big usable number, and verification spend on a list you'll mostly throw away is wasted twice.
- **Treating verification as the whole cleaning step.** It isn't. Dedup, exclusion lists, and the MX gateway check live in `/list-hygiene` and apply to every list, verified or not.
- **Running concurrency above 3.** It throttles, retries pile up, and the run takes longer than it would have at 3.
- **Bulk-verifying a speculative list up front.** Sequence spend by confidence: verify a small batch, send thin, read replies, then pour money behind what's working.
- **Trusting a sending platform's own "unverified" flag** on leads you already verified. Those flags are usually meaningless. Your verifier's verdict is the truth.

## What to do next

Run `/list-hygiene` on the verified output (dedup, exclusions, and the mandatory MX check). Then `/cold-email-copywriting` and `/emailbison-campaign-launch`.

If the invalid rate came back above ~30%, stop and question the source before salvaging the remainder. That list is telling you something.

## Related skills

- `/list-hygiene` - dedup, exclusion lists, MX gateway exclusion, catch-all density
- `/ai-ark-list-builder` - build lists that don't need this skill at all
- `/deliverability-triage` - what to do when bounces spike after sending
- `/emailbison-campaign-launch` - upload the finished list as a draft campaign
