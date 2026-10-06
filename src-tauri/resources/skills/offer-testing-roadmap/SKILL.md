---
name: offer-testing-roadmap
description: Turns one offer into a systematic week-by-week roadmap of offer ANGLES to test, so the campaign is a search for what the market wants instead of a single bet. Builds the default two-base roadmap (Ideas/plan in the Offer First frame plus Case Study in the Built For You frame, with the angle as the only test variable), sets the one-offer-per-week cadence, and defines when an offer gets killed (by sends, never by calendar). Use after /offer-framing has produced candidate offers, or whenever someone says "what should I test next", "build my testing roadmap", "my offer flopped, now what", "how many offers should I run at once", "when do I kill a variant", "plan my next few weeks of campaigns", or "how do I know which offer is winning".
---

# Offer Testing Roadmap

A cold email campaign is not a launch, it is a search. You are searching for the offer angle your market actually bites on, and almost nobody guesses it right first time. I have watched confident first guesses die at two thousand sends more times than I can count. The teams that win are not the ones with the best first guess; they are the ones with a roadmap, so that when week one's offer dies, week two's is already written.

This skill turns the output of `/offer-framing` into that roadmap: a queue of distinct offer angles, a default cadence of one new offer per week, and hard rules for reading winners and killing losers.

## When to use

- Right after `/offer-framing`, before writing final copy
- When a live offer has flopped and the user asks "what now"
- When the user is about to test five framings of the same offer (stop them, this skill explains why)
- At the start of any project, so the whole test plan exists on day one

## The one rule: test OFFERS, not framings

Early on, the variable that matters is which offer, which value proposition, the market bites on. That is the big lever. Which of four wordings of the same offer performs 10% better is the small lever, and testing it early just spreads your volume thin while teaching you nothing about what the market wants.

So: hold the wording steady using two base frames (below), and spend your volume testing many distinct ANGLES. Only after an angle is winning do you bother A/B testing its wording.

An angle is the specific thing the offer is about: one pain, one outcome, one metric, one mechanism. "Reduce hiring time" and "stop losing good applicants to slow replies" are two different angles for the same product. The angle is what you swap; everything else stays fixed.

## The default roadmap: two bases, angles as the variable

When there is no proven offer yet, start here every time. Two complementary vehicles that between them cover the two natural stances:

**Base 1: Ideas / plan (the forward stance, the Offer First frame).**

> "if I shared a few ideas on how you could {angle}, would that be useful?"

Forward-looking value, trivially easy to produce, and you just swap the angle until one bites. The moment this gets interested replies, you have learned the market wants help with that exact thing, which is the entire point of the early phase. Keep it conditional ("if I shared") and match the register to the buyer: "a few ideas" is right for marketing and growth buyers, but reads flippant to a compliance or risk buyer, who should get "the step-by-step plan for {angle}" instead. Same vehicle, tuned register.

**Base 2: Case study (the proof stance, the Built For You frame).**

> "put together a breakdown of how {peer company type} {achieved the angle}. Want to see it?"

Proof that a peer already got the result. It is done, it exists, and the ask is just to look at it. You swap the angle here too: which result, which metric, which mechanism the peer achieved.

Both bases are genuinely strong even when the user has nothing else to offer. If `/offer-framing` found a real no-brainer (a trial, a guarantee, a free service sample), that offer runs as well, as the lead, and usually as the standing control the whole way through. The two bases test around it, they do not replace it.

**Write both frames for every offer.** Each angle on the roadmap gets an Offer First version (the conditional question) and a Built For You version (the already-made artifact). The "leans" above just tell you each base's natural opener; you still write both. The Built For You needs an artifact you can honestly claim already exists, which for a service can be the setup itself, pre-run: "we just set up {the thing} for {COMPANY}" (see `/lead-magnet-builder` for that shape in full).

## Building the angle set

List the product's distinct value propositions: pains killed, outcomes bought, metrics moved, mechanisms that are genuinely unique. Aim for **4 to 7 distinct angles** for the opening roadmap, more if the project runs long. Rules that keep the set honest:

- **Each angle must be a value-prop the buyer perceives unaided.** Not an internal craft you happen to be proud of. A marketing buyer does not think in your production sub-disciplines; they think "this video looks cheap" or "we are drowning in applicants".
- **Cut near-duplicates ruthlessly.** If a yes to angle A would mean the same thing as a yes to angle B, merge them. Two angles that bite on the same value-prop waste volume discovering the same thing twice.
- **A feature is not an angle.** "Works on WhatsApp" is a feature; it rides along as a proof line in a ps, it does not get a week of volume. Lead with the pain it kills or the outcome it buys.
- **Some value-props are objection-handlers, not openers.** Price comparisons ("about half what the legacy vendor charges") sell once someone is interested but do not open. Hold them for the reply stage.
- **Every set includes a case study angle.** Peer proof is the standing default; it earns its slot on every roadmap.
- **Every offer carries relevance and credibility, not just the value question.** The prospect is silently asking two things: why should I listen to you, and why is this relevant to me specifically. So every angle gets a proof line ("we do this for companies like yours, and here is the result they got") alongside the value line.

### Where the angles come from

Five sources, in rough order of yield:

1. **The pains in `profiles/profile.yaml`**: what the ICP section says these buyers struggle with, one angle per distinct pain.
2. **Why the best customers actually bought**: the reason the named best customer signed is almost always a strong angle, and it comes with built-in proof.
3. **The proof points and case studies**: every real result with a number is a case-study angle waiting to be written.
4. **Competitor gaps**: an outcome the whole field talks about is table stakes; an outcome nobody in the space leads with is an angle you can own.
5. **Reply mining from earlier sends**: what non-interested replies complained about or asked for. The market literally tells you the next angle if you read it.

## The cadence: one offer per week, all the volume behind it

The default testing motion is **one new offer per week, per campaign, taking all the sending volume**. Two reasons:

1. Testing ten offers at once spreads volume so thin that nothing reaches a readable sample for weeks, and in the meantime you have learned nothing.
2. One well-produced offer per week means there is always a concrete story: "here is this week's offer, here is how last week's did." If you report to a client or a boss, the update writes itself.

**Spray-many-at-once is valid ONLY as an initial discovery phase.** If you are starting completely cold with zero signal, one opening sweep with several angles loaded as rotating variants is a fair way to get a first read. But it is a phase, not the steady state: run the spread once, consolidate all volume onto whatever earned interested replies, and switch to the weekly one-at-a-time cadence for everything new.

### Retirement is by SENDS, never by time

The calendar is a convenience; the send count is the rule. An offer is only readable once it has real volume behind it:

- **Kill bar:** roughly 2,000 cumulative sends to the offer, around 20 replies, and zero interested. That offer is dead; rotate to the next on the roadmap.
- **If a week passes and the offer has NOT cleared the bar** (volume was split, sending was slow), do not rotate on schedule. Hold it and say so. Rotating an unread offer teaches you nothing and burns a roadmap slot.
- **Reading a winner:** interested replies divided by total replies, per offer. 20%+ is great, 10%+ is good, under 5% is weak. A genuinely interested reply is a yes, an information request, or a real technical question; "who is this" is not interest.

### Winners get concentration, not celebration

When an angle bites: drop the dead angles, push all volume to the winner, and only then A/B its wording, frame, or vehicle. A small sample of interest (even one strong interested reply in a few hundred sends) is a signal to allocate more volume and spin off close variants, not to declare victory.

### Single-send lists recycle

If the list gets one email per contact (no sequence), the roadmap doubles as a recycle queue: after roughly 45 days, run the same list again with the NEXT offer on the roadmap, a fresh subject line, and a different sending address. The angles you did not fit into round one are round two.

## Reading replies beyond the numbers

The rates tell you IF an offer works. The reply text tells you WHY, and it is the cheapest diagnostic you have:

- **Read who replies, not just how many.** An angle pulling founders who want to network, competitors sizing you up, or freebie-hunters is not winning, whatever its reply count. Buyer-pull beats reply volume; a winner attracts people who could actually purchase.
- **Mine the non-interested replies.** "Not right now", "we handle this in-house", "already using {competitor}" are targeting and awareness data. Three "we already solved this" replies means the angle arrived two years late for that segment; that kills an angle as decisively as silence does.
- **Small samples of interest still steer allocation.** One strong interested reply in 500 sends does not conclude a test, but it earns that angle more volume and a couple of close-variant spin-offs. Verdicts need the full sample; allocation decisions do not.
- **An objection that repeats becomes reply-stage material.** If every third reply asks about price, the price answer belongs in your reply playbook, not in the opening email.

## The long version: a 12-week roadmap at project start

For a serious project, build the full plan on day one rather than improvising week to week: roughly twelve angles, one tested per week, mixing straight-to-service offers and magnet-backed ones. It is simultaneously the test plan and the progress report, and it removes the "what do we do now" panic when an angle dies, because the answer is already written down. Two approval gates if you are running this for someone else: they approve the OFFERS first (people who reply will arrive expecting exactly what was promised, so whoever delivers must know and be able to honour it), and the MESSAGING second. Collapsing those two conversations into one loses the first.

## The roadmap table (the actual deliverable)

Produce this table, ordered by conviction: the front-end service offer first if one exists, then the case study, then the sharpest pains:

| # | Angle | Vehicle | Frames written | Status |
|---|---|---|---|---|
| 1 | {the no-brainer, if any} | service offer | Offer First | control, always on |
| 2 | {sharpest pain} | ideas / plan | Offer First + Built For You | week 1 |
| 3 | {peer result} | case study | Offer First + Built For You | week 2 |
| 4 | {second pain} | ideas / plan | both | week 3 |
| 5 | {built asset} | deliverable | both | week 4 |

Statuses move through: queued, live, winner, killed. Update it weekly; it is the memory of the search.

## Example: a fictional roadmap

**Marlow Freight Tech, sells route optimisation software to regional haulage firms.** `/offer-framing` found a 30-day pilot (a real entry point) and proof of "cut empty miles 18% for a 60-truck fleet".

1. **Control:** the pilot itself. "if I could set you up with a 30-day pilot, no cost, and you only keep it if the numbers move, would that be useful?" Runs throughout.
2. **Week 1, angle: empty miles.** Ideas base: "if I shared a few ideas on how you could cut the empty miles your trucks run each week, would that be useful?"
3. **Week 2, angle: peer proof.** Case study base: "put together a breakdown of how a 60-truck fleet cut empty miles by 18% in a quarter. Want to see it?"
4. **Week 3, angle: driver hours.** Same ideas base, new angle: wasted paid hours sitting in loading queues.
5. **Week 4, angle: fuel spend.** A built-asset vehicle: a fuel cost benchmark for fleets their size.

Week 2's case study clears 2,000 sends with a 14% interested rate; weeks 3 and 4 get bumped, all volume goes to peer-proof angles, and only now do we A/B the case study's wording. That is the system working.

## Common mistakes

- **Testing framings instead of offers.** Four wordings of one offer is one test wearing four costumes. Spend early volume on distinct angles.
- **Rotating on the calendar instead of the send count.** An offer that only got 400 sends this week has not been tested. Hold it.
- **Killing on vibes.** "It feels like it's not working" after 300 sends is not a verdict. The kill bar has numbers; use them.
- **Running the discovery spray forever.** Ten parallel offers at 200 sends each, month after month, reads as activity and produces nothing. Consolidate.
- **A roadmap of weak offers.** Twelve mediocre angles is just twelve weeks of losing slowly. Every angle has to sound genuinely valuable on its own; if one would embarrass you to send, cut it before it costs you a week.
- **Forgetting the control.** If a real no-brainer exists, it runs throughout as the benchmark. New angles have to beat it, not just show a pulse.
- **Reading raw reply rate instead of interested rate.** Angry replies and unsubscribes count in reply rate. Interested divided by replies is the number that pays.

## What to do next

With the roadmap table written, take the week 1 offer into `/cold-email-copywriting` to write the actual emails in both frames, house style. If an angle needs a real free asset behind it, build that first with `/lead-magnet-builder`.

## Related skills

- `/offer-framing`: the doctrine that produced the candidate offers this skill sequences
- `/lead-magnet-builder`: builds the free asset when an angle's vehicle is a deliverable
- `/cold-email-copywriting`: writes each roadmap entry as real email copy
- `/reply-classification`: defines the interested-reply buckets the verdicts here depend on
