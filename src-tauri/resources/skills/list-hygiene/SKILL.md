---
name: list-hygiene
description: Final cleaning pass every lead list gets before it touches a sending tool: dedup by email and by company domain, exclusion lists (customers, competitors, past bounces, unsubscribes), and the mandatory MX check that silently drops Mimecast and Barracuda protected domains before any send. Triggers on "clean my list", "dedupe the list", "remove duplicates", "suppression list", "exclusion list", "don't email our customers", "one contact per company", "MX check", "Mimecast", "Barracuda", "why are my bounces so high", "is this list ready to send". Applies to EVERY list from EVERY source, including AI Ark exports. Also covers catch-all density as a list-quality signal.
---

# List Hygiene

Every list, from every source, gets this pass before upload. AI Ark export, verified CRM dump, doesn't matter. Skipping hygiene is how you email your own customers, send four reps at the same company the same "personalised" line, and hand a chunk of your list to mail gateways that were always going to reject you.

Three steps, in order: dedup, exclusions, MX check. Then one diagnostic read.

## When to use

- After `/ai-ark-list-builder` produces an export (always)
- After `/email-verification` on a list you brought from elsewhere (always)
- Before ANY upload to a sending tool (this is the gate)
- When bounces are high and you suspect the list was never cleaned

## Step 1: Dedup

Two levels, and most people only do the first:

**By email.** Obvious. Exact-match after lowercasing and trimming. Do it across EVERY source feeding the campaign, not per file: the same person shows up in your fresh export, your old list, and your CRM, and cross-file duplicates are the ones that actually get double-sent.

**By company domain.** When your play is one-contact-per-company (and for most cold outbound it should be, nothing says "mass blast" like three people at Northwind Traders comparing identical emails at standup), collapse to one contact per domain. Keep the best-fit title, drop the rest. Keep the dropped contacts in a side file; they're your follow-up pool if the first contact goes quiet, and your replacement pool when the first contact bounces.

If you're deliberately multi-threading into large accounts, that's a strategy choice, make it consciously per segment, never by accident.

## Step 2: Exclusion lists

Maintain these as standing files and run every list against them before upload:

- **Customers.** Cold-pitching a paying customer is the single most credibility-destroying email you can send. Export your customer domains regularly and exclude by DOMAIN, not by email address, so new hires at customer companies are covered too.
- **Competitors.** They read your copy, screenshot it, and occasionally reply pretending to be interested. Exclude their domains.
- **Past bounces.** An address that hard-bounced once will bounce again. Feed every campaign's bounce list back into the standing exclusion file. This is cumulative and it compounds: the longer you run, the cleaner your lists get for free.
- **Unsubscribes and opt-outs.** Legally required in most places, and ignoring them is how you get spam-flagged by the one person guaranteed to be annoyed. Same file, forever, across all campaigns.
- **Open conversations.** Anyone currently in an active thread or an active sequence. Nothing kills a warm deal like a cold step-1 email landing mid-negotiation.

One list per category, append-only, checked into your own private store (never a public repo). The exclusion pass is a simple domain-and-email set-difference; keep it boring.

## Step 3: The MX check (mandatory, silent, automatic)

Some corporate mail gateways reject cold mail from new senders at close to 100%, on policy, regardless of how valid the address is. **Mimecast and Barracuda are the two that matter.** Every time I've measured it, they're a small share of the list and a wildly outsized share of all bounces: they reject at the connection level, before your email is even evaluated.

Verification cannot fix this. It's a receiver policy problem, not a bad-address problem, which is exactly why this is its own step and not part of `/email-verification`. AI Ark lists skip verification; they NEVER skip this.

**The rule: check MX records for every domain on the list and silently exclude any domain whose MX points at Mimecast or Barracuda.** Silently means it's automatic, not a judgment call, not optional, and not something you agonise over per lead. Those leads were never reachable by cold email; they cost you nothing to drop and a domain reputation to keep.

```bash
npx tsx ${CLAUDE_SKILL_DIR}/scripts/mx-exclusion-check.ts --csv leads.csv
```

What it does:

1. Dedupes the list to unique domains
2. Runs an MX lookup on EVERY domain (never a sample)
3. Flags any domain whose MX host contains `mimecast` (including country variants) or `barracuda` (for example `*.barracudanetworks.com`)
4. Drops every lead on a flagged domain and writes the clean file
5. Reports the drop count

**Always record the drop count.** A hygiene pass that reports no MX number never ran the check. Expect low single-digit percentages on most B2B lists, higher in industries that love appliance gateways (legal, finance, government-adjacent).

Other gateways are fine: Google, Microsoft 365, and Proofpoint hosted domains all accept properly-warmed cold mail at normal rates. Don't over-rotate and start excluding every security vendor you've heard of. Mimecast and Barracuda, that's the list.

## The diagnostic read: catch-all density

If the list went through verification, look at the share of `valid-risky` (catch-all) verdicts as a quality signal about the SOURCE:

- **~10-25% catch-all:** normal for B2B. Proceed.
- **~25-50%:** leaning stale or scraped. Fine to send, but weight your bounce monitoring toward this list.
- **50%+:** the source is low quality. Catch-all domains are where dead addresses hide (the server accepts everything, so nothing was really confirmed), and a source this heavy in them usually has other problems too: stale titles, wrong people, dead companies. Question the source before scaling anything built on it.

This costs you nothing, the verdicts are already sitting in the verified file. Read them.

## Common gotchas

- **Skipping hygiene on AI Ark exports because "they're verified".** Verification and hygiene are different things. Verified lists still contain duplicates, your customers, and Mimecast domains.
- **Deduping per file instead of across all sources.** The duplicates that embarrass you are cross-source.
- **Excluding customers by email instead of domain.** The contact you have isn't the contact your list build found.
- **Sampling the MX check.** Every domain, every time. The lookup is fast and free; a sample defeats the point.
- **Running the MX check once and reusing the result forever.** Companies change mail providers. Re-check when recycling a list older than a few months.
- **Letting the bounce exclusion file go stale.** Feed bounces back after every campaign or the compounding never happens.
- **Attaching an old lead pool to a new campaign without re-running this pass.** Re-used pools bypass every gate you built into the fresh-list path. If leads are entering a campaign, they go through hygiene, whatever their origin.

## What to do next

The list is now send-ready. Run `/cold-email-copywriting` if the copy isn't written, then `/emailbison-campaign-launch` to upload as a draft, and `/pre-launch-qa` before anything goes live. The QA gate re-checks for gateway-protected domains as a backstop, but the backstop existing is not a reason to skip the real check here.

## Related skills

- `/email-verification` - verify bring-your-own lists first (and never AI Ark exports)
- `/ai-ark-list-builder` - where the list came from
- `/pre-launch-qa` - the final gate before any campaign goes live
- `/deliverability-triage` - if you sent without this pass and bounces are spiking
- `/emailbison-campaign-launch` - upload the clean list as a draft
