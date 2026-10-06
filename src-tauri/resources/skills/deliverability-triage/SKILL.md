---
name: deliverability-triage
description: "Diagnose a bounce or deliverability problem properly before touching anything: pull the actual bounce reasons from EmailBison, break them down by receiving ESP (MX lookup) and SMTP code, cross-reference the verifier verdicts, then pick the ONE right lever. Never reflex re-verify. Triggers on 'bounces are high', 'why are we bouncing', 'emails aren't landing', 'going to spam', 'deliverability problem', 'senders disconnecting', 'bounce rate alert', 'should I re-verify the list'. Also defines the thresholds: act on sustained >6% bounce across 2+ days, ignore one-day spikes, ignore a handful of disconnected senders."
---

# Deliverability Triage: diagnose first, then pick the lever

The reflex when bounces climb is "re-verify the list". Nine times out of ten that's wrong. Re-verification fixes exactly one of the five bounce causes (a genuine verifier miss on a non-catch-all domain). It does nothing for ESP (the receiving mail provider: Gmail, Outlook, a corporate gateway) reputation blocks, catch-all dead mailboxes, spam-word content, or under-warmed domains. Spending money on verification before you know which problem you have is guessing with a credit card.

The rule this skill enforces: **a bounce number without a diagnosis is worthless.** "6% bounce" tells you nothing. "6% bounce, 64% of it from one security ESP that's 4% of the list, all policy-rejection codes" tells you exactly what to do, and it isn't verification.

## When to use this

- Bounce rate looks high on any campaign
- Emails "aren't landing" or replies dried up suddenly
- You're tempted to re-verify a list
- Senders are showing disconnected
- Before ANY reaction to a bounce alert

## The thresholds (know these before you panic)

- **Sustained >6% bounce across 2 or more days** with meaningful volume (25+ sends): investigate. A single-day spike is noise; small denominators make scary percentages. Never act on one bad day.
- **Disconnected senders:** a handful is normal churn, inboxes wobble and reconnect. Ignore up to roughly 10 disconnected senders per workspace. Above that, something structural is wrong (provider issue, auth expiry across a domain) and it's worth a look.
- **EmailBison's "unverified" lead status means nothing.** The platform verifies nothing on upload; every lead arrives "unverified" regardless of quality. Never treat that flag as a signal and never re-verify a list because of it.

## The diagnosis, in order

### Step 1: Pull the actual bounces

Bounce reasons live in EmailBison's replies feed with the server-side filter:

```
GET /api/campaigns/{id}/replies?folder=bounced
```

Paginate to `meta.last_page` (15 per page, always; see `/emailbison-api`). For each bounce capture the recipient (`lead.email`, falling back to the address in the bounce body) and extract the SMTP status code from the body text, the `5.x.y` pattern.

Also get the rate, not just the count: `bounced / emails_sent` from `POST /api/campaigns/{id}/stats`. A count without a denominator means nothing.

### Step 2: ESP breakdown (MX lookup): is one provider over-indexing?

For each bounced recipient's domain, look up the MX record (`dig +short MX domain.com`) and classify the mail provider: Google, Microsoft 365, Proofpoint, Mimecast, Barracuda, and so on.

The tell you're looking for: **an ESP that's a small share of your list but a huge share of your bounces.** That's a connection-level reputation rejection, nothing to do with address validity. Mimecast and Barracuda are the two repeat offenders: they reject new-domain cold mail on policy, not on content. `/list-hygiene` excludes both automatically before anything gets sent, so this pattern should be rare if you ran it. If you're seeing it anyway, the exclusion step got skipped.

- **Mimecast or Barracuda showing up at all: hard-exclude, don't verify.** They reject new-domain cold mail on policy at close to 100%, independent of whether the address exists. Strip every Mimecast and Barracuda lead from the send set and exclude them at list build going forward (see `/list-hygiene`). No verifier can fix a policy rejection.
- One ESP over-indexing but not those two: read Step 3's codes for that ESP. Could be reputation on young domains (warmup problem) or that provider's directory-reject behaviour.
- Bounces spread roughly in proportion to your list's ESP mix: not an ESP problem. Cause is per-address, keep going.

### Step 3: SMTP code breakdown: the true reason family

The code in the bounce body is ground truth. Do not eyeball the prose; words like "policy" and "denied" mislead. Bucket by code:

| Code | Meaning | Family |
|---|---|---|
| `5.1.1` | mailbox doesn't exist | DEAD ADDRESS |
| `5.1.10`, `5.1.351`, `5.0.350` | recipient not found (Microsoft) | DEAD ADDRESS (usually catch-all) |
| `5.4.1` | access denied | mostly DEAD ADDRESS (directory reject) |
| `5.7.x` | policy / sender reputation / IP block | REPUTATION / SPAM |
| `5.2.2` | mailbox full | transient, leave it |

Two very different worlds:

- **5.7.x reputation blocks are the killer.** They mean receiving servers distrust your domains. More sending makes it worse. Levers: check warmup is on, slow down, exclude the blocking ESP, let domains age. Verification is irrelevant.
- **5.1.x dead addresses are tolerated as normal mailflow.** Providers expect some. Each dead address bounces exactly once, then EmailBison suppresses it, so a 5.1.x-heavy rate self-corrects downward over a few days as the deadwood burns off.

### Step 4: Cross-reference the verifier verdicts

Join the bounced addresses against your verification results (Kitt verdicts, for lists you verified; see `/email-verification`):

- **Bounced addresses were mostly `valid-risky` (catch-all):** this is the designed tradeoff of sending to catch-all domains. They accept every address at the SMTP handshake, then bounce the dead ones later. **No verifier on earth can catch these**; re-verifying is burning money. Lever: split valid-risky onto its own campaign, drop it, or let it burn off.
- **Bounced addresses were mostly `valid`:** a genuine verifier miss. This is the one case where second-pass verification with a different verifier is the right lever.
- **`invalid` addresses in the bounces:** invalids leaked past your upload filter. Fix the filter (send `valid` + `valid-risky` only) and purge them. They were already flagged; verification worked, your pipeline didn't.
- **Not in any verification file:** the leads were never verified. Verify them now (unless they're an AI Ark export, which ships pre-verified).

### Step 5: Report the composition, not the headline

Lead with the breakdown: "X bounces (Y%). Reputation blocks: N (n%), concentrated on [ESP]. The rest is dead-address on catch-all domains, Z% of which the verifier had marked valid-risky. Lever: [one action]." One paragraph like that beats any dashboard.

## The decision tree

| Diagnosis | Lever | NOT the lever |
|---|---|---|
| Mimecast/Barracuda over-indexing | Hard-exclude that ESP at list build + purge from live campaigns | re-verify |
| 5.7.x reputation on young domains | Warmup on, lower volume, let domains age | re-verify |
| 5.1.x concentrated in valid-risky | Split or drop valid-risky, or let it burn off | re-verify (catch-all is unverifiable) |
| 5.1.x concentrated in valid | Second-pass verify with a DIFFERENT verifier | doing nothing (this is the one verify case) |
| Invalids present in bounces | Fix the upload filter, purge invalids | re-verify |
| Content triggering blocks | Spam-word scan + rewrite | re-verify |
| High but 5.1.x-heavy and trending down | Watch, it's self-correcting | anything drastic |

One protective action is always safe: pausing. A campaign over 6% bounce with real volume can be paused while you diagnose (`PATCH /api/campaigns/{id}/pause`, always reversible). Pausing buys thinking time; it never makes anything worse.

## Common gotchas

1. **Trusting the bounce prose over the code.** "Access denied" sounds like reputation; `5.4.1` is usually a dead address. Bucket by code.
2. **One-day spike panic.** 3 bounces on 30 sends is 10%. It's also 3 emails. Sustained, 2+ days, meaningful volume.
3. **Re-verifying catch-all bounces.** Structurally impossible for any verifier to catch. Stop paying for it.
4. **Treating EmailBison "unverified" as a quality signal.** It's the default state of every lead. The platform checks nothing.
5. **Counting a handful of disconnected senders as a crisis.** Under ~10 per workspace is Tuesday.
6. **Fixing deliverability while ignoring the list.** The cheapest deliverability lever is usually upstream: cleaner list, ESP exclusions, verification of non-AI-Ark sources.

## What to do next

If the diagnosis points at list quality, run `/list-hygiene` (ESP exclusions, dedupe) and `/email-verification` for lists you brought yourself. If it points at content, rework the copy with `/cold-email-copywriting`. If it points at young domains, slow down per `/sender-scaling` and check warmup in your inbox provider. Before any campaign resumes after changes, run `/pre-launch-qa`.

## Related skills

- `/list-hygiene`: the MX exclusions and hygiene that prevent most of this
- `/email-verification`: when verification is actually the lever (and when it isn't)
- `/sender-scaling`: the volume discipline that prevents reputation blocks
- `/emailbison-api`: pagination and endpoint mechanics used in the pull
- `/pre-launch-qa`: the gate that catches these problems before launch
