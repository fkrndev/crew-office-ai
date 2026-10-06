---
name: reply-classification
description: Classify cold email replies into the right buckets and compute the one metric that tells you if your campaign is working. Triggers on "classify this reply", "is this reply interested", "what bucket is this", "sort my replies", "count my interested replies", "what's my interested reply rate", "how are my replies looking", "a reply came in", "is this a positive reply", "are these replies good", "reply rate benchmarks", "how many interested replies should I have". Use whenever a reply lands and you need to decide what it is, or when you're reviewing a batch of replies to judge campaign health.
---

# Reply Classification

Every reply gets a bucket. Not vibes, a bucket. Classification is the first thing that happens when a reply lands, because everything downstream depends on it: what you send back, whether you follow up, and whether your campaign is actually working or just making noise.

I classify strictly. The single most common mistake I see is counting soft, polite replies as "interested" and then wondering why none of them book. A generous classifier makes your numbers look great and your calendar look empty.

## When to use this

- A reply just landed and you need to decide what to do with it
- You're reviewing a week of replies and want an honest read on campaign health
- You want to know if your interested rate is good, bad, or fix-it-now bad
- Someone (you, a teammate, Claude) is about to call a "sounds interesting, maybe next quarter" reply a win and you want to check

## The six buckets

1. **Interested.** Strict definition, see below. This is the only bucket that counts toward your interested-reply rate.
2. **Not interested.** A no, in any politeness wrapper. "Not for us", "we're all set", "please don't contact me again" without an explicit unsubscribe demand.
3. **Objection.** They engaged with the offer but pushed back on something specific: price, timing, an existing vendor, credibility. An objection is not a no. It's a question wearing armour.
4. **Out of office.** Auto-replies, parental leave, "I'm travelling until the 14th". No human read your email yet.
5. **Unsubscribe.** An explicit removal request. "Take me off your list", "unsubscribe", legal-flavoured versions of the same. Honour it immediately, every time, no exceptions.
6. **Bounce / auto.** Delivery failures, "this person no longer works here" auto-responders, ticket-system receipts, security scanners. Not a human.

## The strict definition of interested

Only three things count as interested:

1. **A clear yes or accept.** "Yes, send it over." "Sure, let's talk." "Sounds good, how do we start?"
2. **An information request.** "How does the pricing work?" "Can you send more detail on how this works?"
3. **A technical question.** "Does this integrate with HubSpot?" "Do you support SOC 2 environments?"

That's the whole list. Everything else is not interested, no matter how polite it sounds.

**Explicitly NOT interested, even though people love to count them:**

- **"Maybe later" and every variant.** "Circle back in Q3", "not right now but keep me posted". That's a soft brush-off, not interest. Classify as not interested (or objection if they gave a real reason you can work with).
- **Polite brush-offs.** "Thanks for reaching out, we'll keep you in mind." Nobody keeps you in mind.
- **Referrals.** "Speak to our head of ops instead" is a handoff, not an interested reply. Useful, act on it, but it does not count in your interested rate.
- **Pitch-backs.** They replied to sell YOU something. Not interested.
- **OOOs.** Obviously. But when auto-replies leak into your counts your rate inflates fast.

## Decision table

All example companies below are fictional.

| Reply | Bucket | Why |
|---|---|---|
| "Yes, interested. What does this look like?" | Interested | Clear yes plus info request |
| "Send over the breakdown you mentioned." | Interested | Accepting the offer made in the email |
| "How does pricing work for a team of 40?" | Interested | Information request |
| "Does this work if we're on Salesforce, not HubSpot?" | Interested | Technical question |
| "Sounds interesting, maybe reach out again in the autumn." | Not interested | "Maybe later" is a brush-off |
| "Thanks, but we handle this in-house." | Not interested | Polite no |
| "We already work with an agency for this." | Objection | Engaged, specific pushback, workable |
| "What makes you different from Meridian Growth Co?" | Objection | Credibility challenge, still in play |
| "This seems expensive for what it is." | Objection | Price pushback, they read the offer |
| "You'd want to talk to Priya, she runs partnerships at Fernwood." | Not interested (referral) | Handoff. Action it, don't count it |
| "I'm out of the office until 22nd September." | Out of office | No human read it |
| "Remove me from this list immediately." | Unsubscribe | Explicit removal. Honour it now |
| "Delivery has failed to these recipients." | Bounce / auto | Machine, not a person |
| "Thanks for contacting Bluepine support, your ticket is #4482." | Bounce / auto | Ticket system |
| "Actually, we do outbound for SaaS companies too, want to chat about OUR service?" | Not interested | Pitch-back |

## The rates that matter

Interested-reply rate = interested replies as a share of total human replies received (strip bounces, OOOs, and auto-responses from the denominator first).

- **20%+ is great.** Offer and list are dialled. Scale what's working.
- **10%+ is good.** Healthy campaign. Keep testing angles to push it up.
- **Under 5% means the offer or the list is wrong.** Not the send volume, not the subject line font. People are replying but nobody wants the thing. Fix the offer or fix who you're sending to.
- **0% with real volume means pause and fix.** If you've sent a couple of thousand emails, got replies, and not one is interested, stop sending. More volume on a broken offer just burns list and domain reputation.

## Step-by-step process

1. **Pull every reply since your last pass.** Don't cherry-pick.
2. **Strip the machines first.** Bounces, OOOs, ticket receipts, unsubscribe processing. These leave the denominator.
3. **Classify each remaining reply** using the decision table. When torn between interested and something else, ask: did they clearly accept, ask for information, or ask a technical question? If you have to squint, it's not interested.
4. **Action the buckets.** Interested and objections get a reply (see reply-drafting). Unsubscribes get removed immediately. Referrals get a new, separate outreach to the named person. Not interested gets a graceful one-line close-out or silence.
5. **Compute the rate** and check it against the thresholds. Under 5%, stop tweaking copy and look hard at the offer and the list. Zero with volume, pause the campaign.

## Worked example

Fictional campaign for Kestrel Analytics, 2,400 sends, 41 replies. After stripping 12 OOOs and 6 bounces: 23 human replies. Of those, 4 were a clear yes or a question about how it works, 3 were objections, 2 were referrals, 14 were polite nos and "maybe later"s.

Interested rate: 4 of 23, roughly 17%. That's between good and great. The right move is to answer the 4 fast, work the 3 objections, start fresh threads to the 2 referral names, and scale the campaign. If those 4 had been 0, the same numbers would say pause and rebuild the offer.

## Common mistakes

- **Counting "maybe later" as interested.** The rate looks healthy, bookings say otherwise. Strict or it's useless.
- **Counting referrals as interested.** A handoff is a new lead, not a won reply.
- **Leaving OOOs and auto-replies in the denominator** (or worse, the numerator). Machines don't buy.
- **Treating objections as rejections.** "We already have an agency" is an invitation to differentiate, not a no.
- **Reacting to one day of replies.** Classify in batches, judge the rate on real volume, not this morning's three replies.
- **Ignoring the 0% signal.** Sending more into a campaign with volume and zero interest is the most expensive way to learn nothing.

## What to do next

Interested replies and objections need answers, fast. Run `/reply-drafting` to write them. If your interested rate is under 5%, the problem is upstream: run `/offer-framing` before you touch another reply.

## Related skills

- `reply-drafting`: write the replies that turn interested into booked
- `reply-followups`: the 8-touch system for interested replies that go quiet
- `offer-framing`: where to go when the rate says the offer is wrong
- `cold-email-copywriting`: the copy that earns the replies in the first place
