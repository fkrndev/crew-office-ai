# 07: Email Verification

Bad addresses bounce, and bounces torch the sending domains you just spent three weeks warming. So verification matters. But the most common verification mistake I see isn't skipping it. It's paying for it twice.

This doc is the condensed version. The full skill is `/email-verification`.

---

## The rule that saves you money

**AI Ark exports are verified at source. Never re-verify them.**

If your list came out of `/ai-ark-list-builder`, skip verification entirely and go straight to `/list-hygiene`. Running an already-verified list through a verifier adds cost and time and changes nothing. Anyone telling you to "always verify everything" is selling verification credits.

**Kitt is strictly for lists you bring from elsewhere:**

- Scraped lists (any source)
- Purchased lists (especially these, they're the stalest)
- CRM exports (a two-year-old contact is a coin flip; people change jobs constantly)
- Old campaign lists you're recycling
- Client-supplied or partner-supplied CSVs

Outside lists routinely run 10-30% invalid. Sending to that burns your domains for weeks. For those lists, the verification spend is a rounding error next to the damage it prevents.

One clarification so you don't over-apply the rule: the MX gateway check in `/list-hygiene` is NOT verification. Verification asks "is this address real". The MX check asks "will this gateway accept our mail at all". AI Ark lists skip verification; they never skip the MX check.

---

## Running Kitt

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/email-verification/scripts/kitt-verify.ts --csv leads.csv [--cap 500] [--email-column email]
```

Output: `verified-leads.csv` (your rows plus a `verdict` column) and `.kitt-state.json` (the verdict cache that makes re-runs free).

The script has the safety behaviour built in, and each piece matters:

- **Concurrency 3, maximum.** Kitt's realtime API throttles hard above that. Cranking concurrency doesn't make the run faster, it makes it fail.
- **Capped per run** (default 500 addresses). It tells you the count before it spends. On a big list, run a small batch first, sanity-check the verdict distribution, then let the rest go.
- **Resumable.** State is written to disk after every chunk. If the run dies (network, credits, Ctrl-C), re-running skips everything already verified. You never pay twice for the same address.
- **Retries with backoff.** Transient failures retry automatically. And it knows the 402 trap: Kitt returns 402 for both throttling AND out-of-credits, and credit exhaustion sometimes arrives as a raw 500. The script reads the body to tell them apart, backs off on throttles, and stops gracefully (state saved) when you're out of money. Top up and re-run to resume.

Every call costs a credit. That's why the cap and the resume cache exist.

---

## Reading the verdicts

| Verdict | Meaning | What I do |
|---|---|---|
| `valid` | Mailbox confirmed | Send. This is your core list. |
| `valid-risky` | Catch-all domain: the server accepts everything, so the individual mailbox can't be confirmed | Judgment call, see below |
| `invalid` | Mailbox confirmed dead | Drop, always. No exceptions, no "but it's the perfect prospect". |

**The valid-risky call** is genuine judgment, not a rule:

- Fresh infrastructure, domains still young: **exclude them.** You have no bounce budget to gamble.
- Established infrastructure, healthy bounce rates, a list that's thin without them: **include them**, but watch bounces daily and pull the segment the moment they trend toward the 6% danger line.
- The middle path I like: send to `valid` first, hold `valid-risky` as a second wave once the campaign has proven itself.

Also read the valid-risky SHARE as a signal about the source. A normal B2B list runs maybe 10-25% catch-all. A list coming back 50%+ valid-risky is telling you the source was junk; treat everything else about it with suspicion too.

If the invalid rate comes back above ~30%, stop and question the source before salvaging the remainder.

---

## Cost discipline

- Never bulk-verify a speculative list up front. Verify a small batch, send thin, read replies, then pour money behind what's working.
- Sample a bought or recycled list BEFORE verifying: check geography, check how many domains are obviously off-ICP. A big raw number is not a big usable number.
- Verification is not the whole cleaning step. Dedup, exclusion lists, and the mandatory MX check live in `/list-hygiene` and apply to every list, verified or not.
- Ignore your sending platform's own "unverified" flag on leads you already verified. Those flags are meaningless (EmailBison verifies nothing on upload). Your verifier's verdict is the truth.

---

## What to do next

Verified output goes through `/list-hygiene` (dedup, exclusions, the MX check), then on to copy and campaign build. Next doc: `08-emailbison.md`.

Full detail: `/email-verification`. Related: `/list-hygiene`, `/ai-ark-list-builder`, `/deliverability-triage`.
