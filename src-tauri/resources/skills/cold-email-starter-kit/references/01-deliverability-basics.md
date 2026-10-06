# 01: Deliverability Basics

Read this once before buying anything. Every rule in this kit that feels slow or paranoid traces back to something on this page. Deliverability is mostly infrastructure and discipline; copy only matters once the email actually lands in an inbox.

## Why cold email never touches your real domain

Your main domain carries your business's entire email reputation: invoices, support, contracts, password resets. Cold email, however well done, generates some spam complaints; that is the nature of emailing strangers. If those complaints stain your main domain, your REAL mail starts landing in junk, and unstaining a domain is somewhere between slow and impossible.

So cold email runs on separate, cheap, lookalike domains. If your company is `acme.com`, you send from `tryacme.com`, `getacme.com`, `acmehq.com`. Prospects who reply still reach a real inbox; your main domain never sends a single cold email. And if a sending domain ever gets burned, you retire it and replace it for about $12 instead of losing your business's mail.

This isn't a hack. It is how every serious cold email operation works.

## The anatomy of a send

When your email hits a receiving server (Google, Microsoft, or a corporate gateway, a company's own mail filter sitting in front of their inbox, like Mimecast or Barracuda), the spam decision weighs roughly five things:

1. **Sender reputation.** Has this domain and inbox sent mail people engaged with before?
2. **Authentication.** Do SPF, DKIM, and DMARC check out?
3. **Volume pattern.** Is this sender ramping like a human or blasting like a bot?
4. **Content.** Links, spam words, tracking pixels, HTML junk.
5. **Engagement history.** Do recipients reply to this sender, or delete and report?

You control all five. The defaults in this kit (1/day per sender, no links, no tracking pixel, plain text, warmup) are each aimed at one of these dials.

## SPF, DKIM, DMARC in plain English

Three DNS records that prove you're allowed to send as your domain:

- **SPF**: a public list of servers permitted to send mail for the domain. Mail from anywhere else fails the check.
- **DKIM**: a cryptographic signature on each message proving it wasn't tampered with and really came from the domain.
- **DMARC**: the policy that tells receivers what to do when SPF or DKIM fails (reject, quarantine, or just report).

Without all three, modern receivers treat you as unauthenticated and you're in the junk folder before content is even read.

**The good news: Zapmail sets all three automatically** when you provision inboxes through it. You don't edit DNS by hand in this system. But know what the records are, because when something breaks, "is authentication passing?" is the first diagnostic question, and you can check it yourself:

```bash
dig TXT yourdomain.com            # should include "v=spf1 ..."
dig TXT _dmarc.yourdomain.com     # should include "v=DMARC1 ..."
```

## Warmup: why 2-3 weeks, and why forever

A brand-new inbox has zero history. If its first act is sending 30 cold emails a day, it looks exactly like what spammers do, because it is exactly what spammers do: fresh domain, sudden volume, no engagement. Filters are built to catch precisely that shape.

Warmup fixes this by generating realistic traffic first: automated emails between your inboxes and a pool of others that get opened, replied to, and moved out of spam. Over 2-3 weeks this builds the engagement history that makes receivers treat your inbox as belonging to a human.

Two rules:

1. **Minimum 2 weeks before any real send, 3 is better.** There is no shortcut. An under-warmed inbox "works" in the sense that it sends; it does not work in the sense that anyone receives.
2. **Warmup never turns off.** It runs underneath your real sending forever, as background reputation maintenance. Positive engagement is a signal you should never stop feeding the receivers.

Use the warmup window. Offer, copy, sequence, list: all of it happens while the inboxes cook, so launch day is the day warmup ends.

## Volume math: senders govern volume

The unit of sending capacity is the sender, not the campaign. Per-sender daily limits stay low and fixed; total volume comes from how many senders you run.

- **Every new sender starts at 1 real email per day.**
- **A sender earns 3/day** when the campaign it serves shows proven engagement: a strong reply rate with genuine interested replies in it. Engagement is the unlock, never the calendar.
- **Want more volume? Add senders.** Never crank per-sender limits. Per-sender volume is the number one spam heuristic you directly control, and reputation damage is per-domain, so a fleet of low-volume senders also spreads the risk. One throttled domain out of 40 costs you 2.5% of capacity, not everything.

The fleet math at the standard 20 domains x 2 inboxes:

| Stage | Per sender | 40 senders total |
|---|---|---|
| Warmup (weeks 1-3) | 0 real sends | 0/day |
| Proving (from week 3-4) | 1/day | 40/day |
| Proven | 3/day | 120/day |

Campaign-level send limits in your sending tool stay HIGH (effectively uncapped) so they never become the bottleneck; the senders are the governor. If volume looks low, count connected senders before touching any other setting.

120/day sounds small next to the "10,000 emails a day" crowd. It compounds: it's ~2,600 sends a month at near-zero reputation risk, and scaling is linear, 20 more domains doubles it. The blast crowd rebuilds their infrastructure from zero every few months. You won't.

## Inbox rotation

Your sending tool spreads each day's sends across all attached senders automatically. This is why every campaign gets ALL senders attached: rotation keeps any single inbox's fingerprint small and means one flagged inbox never stalls the campaign. Don't hand-assign leads to specific inboxes, and don't override rotation without a measured reason.

## What burns reputation, and how to recover

**The usual causes, in order of frequency:**

- High bounce rate from a dirty list (this is why hygiene and the MX check are mandatory)
- Volume spikes: skipped warmup, raised per-sender limits, or a big list dumped on young senders
- Spam-shaped content: links in a first touch, tracking pixels, spammy words
- Sending into corporate gateways (Mimecast, Barracuda) that reject cold mail on policy, which shows up as connection-level bounces
- Ignoring engagement: months of sends nobody answers is itself a negative signal

**The signs:** bounce rate climbing past ~6% and staying there for multiple days (one-day spikes happen; sustained rates are the signal), reply rate collapsing to zero on a list that used to answer, or test sends to your own accounts landing in spam.

**The recovery, and this is the part people resist:**

1. **Stop sending on the affected domains. Immediately.** Every additional send digs the hole deeper.
2. **Diagnose before acting.** Pull the actual bounce reasons and find the cause: list, volume, content, or gateway. Fixing the wrong thing while continuing to send is the expensive version of doing nothing. `/deliverability-triage` is the full process.
3. **Wait.** Warmup-only traffic for 2-4 weeks can partially rehabilitate a lightly stained domain.
4. **Mostly: replace.** A properly burned domain is not worth rescuing. Retire it, buy a fresh one, warm it, carry on. This is exactly why the system runs many cheap domains instead of one precious one, and why recovery is a line item, not a crisis.

The honest summary: reputation is easy to keep and miserable to win back. Every boring rule in this kit exists so you never meet step 4.

## Next

`02-offers-and-strategy.md`: what the emails should actually offer, which matters more than everything else in this kit combined.
