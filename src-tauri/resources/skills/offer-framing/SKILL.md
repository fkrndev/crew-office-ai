---
name: offer-framing
description: The offer doctrine, the single highest-leverage skill in this repo. Decides WHAT your cold emails actually offer before a word of copy gets written: distance-from-point-of-sale framing, the decision stack (front-end the service itself first, fall back to a lead magnet second), why the no-cost element is the strongest lever, and why offers beat copy every time. Use when starting a new campaign after /icp-builder, when reply rates are flat, or whenever someone says "my cold email isn't working", "nobody is replying", "what should I offer", "my offer isn't landing", "should I give something away for free", "is my offer good enough", "help me build an offer", or "people open but don't reply". Run this BEFORE writing or rewriting any copy.
---

# Offer Framing

Most cold email advice is about copy. Most cold email failure is about the offer. Copy cannot fix a bad offer; it only sharpens a winning one. When a campaign is flat, the offer is the first place I look, and it is usually the answer.

This skill takes the `profiles/profile.yaml` built by `/icp-builder` and produces the offer (or small set of offers) your campaign will run on. It is doctrine plus a process, in that order, because if you understand why the doctrine works you will stop reaching for the generic "free audit" that every prospect has learned to ignore.

## When to use

- Right after `/icp-builder`, before any copy exists
- When replies are flat or the interested rate is under 5% and volume is not the problem
- When the user is about to write copy for a raw service pitch ("we do X, interested?")
- Any time someone asks what to give away, guarantee, or lead with

## The one principle everything hangs off

**The one who wins cold email is the one who gives away the most genuine value for free, upfront.**

Nobody says yes to paying off a cold email. A stranger with no trust and ten other pitches in their inbox needs a reason to reply that costs them nothing and risks nothing. So the question is never "what do I sell them". It is "how much genuinely pay-worthy value am I willing to hand over before they have given me anything?" How much you will give away sets the ceiling on every offer you can build.

Two consequences:

- **The free thing has to be genuinely pay-worthy.** Something they would actually pay for, not a templated PDF or a generic AI report. A cheap-feeling freebie is worse than no freebie: it signals what working with you is like.
- **The no-cost element is the strongest lever you have.** If there is any free element available (a trial, a free sample of the work, a free first step), it belongs in every variant you send. Phrase it "on the house" or "no cost" rather than hammering the word "free" repeatedly.

## The spectrum: distance from the point of sale

Every possible offer sits somewhere on a line measured by how far it is from the thing you actually sell:

1. **The service itself** (closest, best). You offer what you sell: free, on trial, on performance, or with a guarantee. Strongest because it gives away the most, and the people who say yes arrive already wanting roughly the conversation you are used to having. Selling the resulting calls is easy.
2. **A deliverable tied to the service** (middle). A free sample of the work, a case study walkthrough, a plan, a built asset. Not the service, but adjacent and genuinely valuable. This is the lead magnet world.
3. **An unrelated incentive** (furthest, almost never right). A gift card, a generic giveaway. It buys the wrong yes and teaches you nothing about the buyer.

The distance controls a trade-off. Further from the sale is more attractive to a cold stranger, but the calls it produces need more real selling: you are moving someone from "mildly curious" to "buyer", not taking an order. Closer to the sale converts calls easily but is often too commoditised to earn a reply in the first place.

**The skill is finding the closest-to-sale offer that a cold prospect will still act on.** Start close, move out only when forced.

## The decision stack (run in order, never skip step 1)

### Step 1: can you front-end the service itself?

You can lead with the service directly if EITHER of these is true:

- **(a) A no-brainer risk reversal exists.** A real guarantee, a performance or pay-on-results deal, a free trial, or doing the service itself free out of sheer confidence. This beats anything you could construct, because it gives away the most. Check `offer.risk_reversal` in the profile, and if it is empty, ask directly: "what could you guarantee, or give away, and stand behind it?"
- **(b) A genuinely low-commitment entry point exists.** A cheap tier, a pilot, a single small project, usage-based pricing. Something a cold prospect could say yes to without a leap. Check `offer.entry_point` and `offer.pricing_model`.

If either holds, frame the service and you are largely done. The script:

> "if I could {service, with the result and rough timeframe}, {risk reversal: on the house until it works / pay only on results / guaranteed or you don't pay}, like we did for {proof}, would that be useful?"

One warning: a raw service pitch with NO risk reversal and NO low entry point ("we do web design, interested?") dies on cold traffic. Cold email is demand generation, not demand capture. These people were not searching for you. A commodity pitch with a thousand lookalike competitors gives them no reason to reply. If that is all you have, you are in step 2.

### Step 2: no no-brainer? Move the free value onto a lead magnet

If the service cannot carry a risk-minimiser, put it on a free deliverable instead: a case study walkthrough, a set of ideas, a plan, a built asset. The free lead magnet IS the risk reversal, because it costs the prospect nothing and carries no downside. That is precisely what makes it attractive when the service alone cannot be.

The script barely changes:

> "if I could give you {the lead magnet} to {result}, free, like we did for {proof}, would that be useful?"

Only the middle slot moves. You are not rewriting the offer logic, just relocating where the safety sits. Building the magnet itself is its own craft: that is `/lead-magnet-builder`.

Every free offer is one of three types. Name which one yours is; if it fits none, it is probably an unrelated incentive and you should be suspicious of it:

1. **Reveal a problem.** An asset that exposes a problem they have: a benchmark, a teardown, "the 7 mistakes {ICP} make when {doing X}". The diagnosis is the value; your service is the cure.
2. **Trial or sample of the solution.** A piece of the actual thing, free. The question to push on: how MUCH can you give away? More is stronger.
3. **One step of a multi-step.** Your service has a natural sequence; deliver the first step free. Real standalone value, and they are one step into your process.

## What the words should be: proven asks vs dead asks

Concrete, tangible deliverables pull. Generic, effortful, sales-shaped asks die. From live campaigns across my client base:

- **Working:** "can I send you a video?", "can I give you the strategy doc?", "can I send you the playbook?", "can I show you how we did it?"
- **Dead:** "can I make you an audit?", "can I give you a free consultation?"

An "audit" signals work and is saturated. A "consultation" reads as a sales call in disguise, because it is one. Both ask the prospect to commit before any value lands. Lead with a specific thing you will hand over, not a process they have to sit through.

And never announce what the free thing is NOT. Hedges like "the thinking, not the full build" or "a rough sketch, not the real thing" strip the value out of the pitch. Pitch a concrete, genuinely good version of the thing, and protect your time in how deep you go on delivery, never in the copy.

## The diagnosis order (when a live campaign is flat)

Volume is fuel, not the fix. Sending more of an offer nobody wants just gets you ignored at scale. When results are thin, diagnose in this order:

1. **Offer value.** Would the ideal prospect genuinely want this? Ask the brutal question: "why would the person reading this say no?" If you cannot answer cleanly, the offer is not sharp enough. Fix this first; nothing downstream matters until it is fixed.
2. **Framing.** The offer is good but the email is confusing, wordy, or buries it. This is a copy-clarity fix, not an offer change. Check the wording before concluding the offer is dead.
3. **Audience.** The offer and framing are right but pointed at the wrong people. Re-check the list against `profiles/profile.yaml`.

Also read WHO replies, not just how many. An offer that pulls networkers, deflectors, and competitors is not working, however many replies it gets. Buyer-pull beats reply volume.

## The three extracting questions

When the profile's offer section is thin (and it usually is), do not brainstorm at the user; interview them. These three questions pull stronger offers out of people than any archetype list, because the owner always knows things about their delivery confidence that never made it onto the website:

1. **"What could you guarantee, or be confident enough to deliver, that you would win business off the back of it?"**
2. **"What could you give away for free that you are so sure is good, it wins you the deal?"**
3. **"Where do you see the business growing?"** New offers that point at the growth direction get delivered with enthusiasm, and they usually widen the addressable market too.

Then reflect their raw answer back as a testable offer: name it, state what gets handed over, and confirm they can deliver it cheaply at volume. Aim to leave the conversation with two candidates, not one; a free foot-in-the-door plus a guarantee is the classic pair.

## Guarantee design (when step 1a is in play)

A guarantee only works if it is built properly:

- **Guarantee something measurable and third-party-verifiable.** "Your page speed score goes from red to amber, check it yourself in the public tool" beats any vague outcome promise. Self-serve proof is what makes a bold claim credible instead of suspicious.
- **Differentiated, and tied to what the buyer actually wants.** "Great customer service, guaranteed" is wallpaper. Guarantee the thing they are measured on.
- **Contingencies are allowed.** "If you don't do X and Y during the process, the guarantee doesn't apply" is fair and protects you from bad-faith claims.
- **Keep the terms off the first email.** Email one carries the headline promise; the conditions come out on the call, after discovery. You can also choose to extend the full guarantee only to great-fit prospects, decided live.
- **Only guarantee what you would honour without resentment.** The maths usually works: closing ten with one refund beats closing five with none. But a guarantee you would dread paying out poisons the relationship and your delivery. If the user will not stand behind it, drop it and move on.
- **Free work needs no guarantee.** It is already free; do not stack risk reversals.

## Steps (what I actually do)

1. **Read `profiles/profile.yaml`.** If it does not exist, run `/icp-builder` first. The fields that matter here: `offer.pricing_model`, `offer.entry_point`, `offer.risk_reversal`, `proof_points`, `case_studies`.
2. **Classify the channel.** Standard cold email is demand generation: you create the want from scratch, so a commodity pitch dies. The rare exception is a signal-based campaign (e.g. emailing companies actively hiring for the role you replace), where the signal manufactures relevance and a straighter pitch can stand.
3. **Run the decision stack.** Step 1: hunt hard for a service front-end. Ask the user the guarantee question and the entry-point question even if the profile looks empty; most people undersell what they could give away. Step 2: if genuinely nothing, move to a lead magnet and hand the construction to `/lead-magnet-builder`.
4. **Draft the offer statement(s).** For each candidate, one sentence in the script skeleton, naming the free element, the result, and the proof. Say which of the three free-value types it is.
5. **Stress-test.** For each: why would the ideal prospect say no? Can the user actually deliver it if 50 people say yes this month? Will the user stand behind the guarantee without resenting it? Kill anything that fails.
6. **Output.** The chosen offer (or 2 to 3 candidates), each with its script-skeleton sentence, ready for `/offer-testing-roadmap` to sequence and `/cold-email-copywriting` to write properly.

## Example: running the stack on a fictional company

**Bramble & Co, a bookkeeping firm for UK hospitality groups.** Profile says: monthly pricing, no trial, best proof is "found a 40-site pub group £60k of unclaimed VAT".

- Step 1a, risk reversal? Asked the founder: "would you do the first month free if you are confident?" She would not. But she WOULD stand behind "if we don't find you at least one material saving in the first review, you don't pay". That is a real, differentiated guarantee. Step 1 passes.
- Draft: "if I could take bookkeeping fully off your plate and guarantee we find at least one material saving in the first review, like the £60k of unclaimed VAT we found for a 40-site pub group, would that be useful?"
- Stress-test: why would they say no? Switching accountants feels painful. So the copy needs to neutralise switching pain, and a second candidate goes on the roadmap: a free "VAT leak check" (a reveal-a-problem magnet, one step of the service) for prospects not ready to switch.

Two offers, one close to the sale and one a step further out, both anchored in real proof. That is the output.

## Common mistakes

- **Jumping straight to lead magnets.** Step 1 exists because a real guarantee or trial beats any lead magnet you could build. Most people never ask the client (or themselves) what they could actually stand behind.
- **The generic free audit or free consultation.** Saturated, effort-signalling, and everyone knows it ends in a pitch. If the value genuinely is diagnostic, package it as a specific named deliverable instead.
- **Pitching the service raw because "our work speaks for itself".** On cold traffic it does not get the chance to speak. No demand, no trust, no reply.
- **A guarantee that is wallpaper.** "100% satisfaction guaranteed" is not a risk reversal. Guarantee a measurable outcome the buyer actually wants, with sensible contingencies, and keep the fine print for the call, not the first email.
- **Fixing copy when the offer is the problem.** Ten rewrites of an offer nobody wants is ten wasted weeks. Diagnose in order: offer value, then framing, then audience.
- **Confusing attractiveness with quality.** An offer far from the sale can pull loads of replies from people who just want free stuff. If you cannot sell the resulting calls, it was not a better offer.

## What to do next

Take the offer (or candidates) from this skill into `/offer-testing-roadmap`. One offer is a bet; a roadmap of angles is a system. That skill sequences what gets tested when, so every week has a plan.

## Related skills

- `/icp-builder`: builds the profile.yaml this skill reads; run it first
- `/lead-magnet-builder`: constructs the free asset when the decision stack lands on step 2
- `/offer-testing-roadmap`: sequences offer angles into a week-by-week test plan
- `/cold-email-copywriting`: turns the chosen offer into actual emails, house style and all
