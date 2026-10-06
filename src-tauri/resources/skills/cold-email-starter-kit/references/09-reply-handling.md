# 09: Reply Handling

The campaign's only job was to get a human to respond. Everything that makes money happens after the reply. Most people put 95% of their effort into the cold send and then fumble the response, either by gatekeeping (withholding the promised thing to force a call) or by going quiet themselves.

The reply system is three parts: classify, draft, follow up. This doc is the condensed version; the full skills are `/reply-classification`, `/reply-drafting`, and `/reply-followups`.

---

## Part 1: Classification

Every reply gets a bucket. Not vibes, a bucket:

1. **Interested** (strict definition below, the only bucket that counts toward your rate)
2. **Not interested** (a no, in any politeness wrapper)
3. **Objection** (engaged but pushed back on price, timing, an existing vendor, credibility; an objection is a question wearing armour, not a no)
4. **Out of office** (no human read it yet)
5. **Unsubscribe** (explicit removal request; honour it immediately, every time)
6. **Bounce / auto** (delivery failures, ticket systems, "no longer works here"; machines, not people)

### The strict definition of interested

Only three things count:

1. **A clear yes or accept.** "Yes, send it over." "Sure, let's talk."
2. **An information request.** "How does pricing work?"
3. **A technical question.** "Does this integrate with HubSpot?"

That's the whole list. Explicitly NOT interested, even though people love to count them: "maybe later" and every variant (that's a brush-off), polite "we'll keep you in mind" replies, referrals ("talk to our head of ops" is a handoff, act on it but don't count it), and pitch-backs. If you have to squint, it's not interested. A generous classifier makes your numbers look great and your calendar look empty.

### The rates that matter

Interested-reply rate = interested replies as a share of total HUMAN replies (strip bounces, OOOs, and auto-responses from the denominator first).

- **20%+ is great.** Offer and list are dialled. Scale.
- **10%+ is good.** Healthy. Keep testing angles.
- **Under 5% means the offer or the list is wrong.** Not the send volume, not the subject line. Fix the offer or fix who you're sending to.
- **0% with real volume: pause and fix.** More volume on a broken offer just burns list and domain reputation.

---

## Part 2: Drafting the reply

The rules, compressed:

1. **Reply fast.** Interest decays by the hour. A mediocre reply in 10 minutes beats a perfect reply tomorrow.
2. **Deliver the promise.** If your cold email offered findings, ideas, an audit, and they said yes, the reply DELIVERS it, in full, in the reply itself. Never "let's hop on a call and I'll walk you through it". That's a bait-and-switch and they feel it instantly. Give the goods; the call becomes the natural way to go deeper. Generosity books more meetings than gatekeeping ever has.
3. **Soften the time ask.** The cold email never mentioned a call (that's copy doctrine). The reply is where the call enters, gently: "only takes 10 minutes", "happy to walk you through it whenever suits".
4. **Propose two exact time slots.** "When works for you?" creates work, and work kills momentum. Two specific real slots, one near, one a few days out, plus a catch-all link: "Does tomorrow at 9am or Thursday at 2pm work (London time)? If neither, grab another time here."
5. **The calendar link is visible and clickable.** A proper hyperlink on natural anchor text ("grab a time here", "pick a slot here"). Never a bare pasted URL. This is the opposite of cold email, where links never appear: post-reply, you've earned the link.
6. **Match their energy and length.** Three-word reply gets a punchy answer; a technical paragraph gets a considered one. 100 words is the ceiling for most replies, under 50 is usually better.
7. **Never over-templatize.** If your reply would work pasted under any other prospect's message, it's not done.

`/reply-drafting` has worked examples for every scenario: hard yes, info request, technical question, soft interest, objection.

---

## Part 3: The 8-touch follow-up system

An interested reply that goes quiet is not a dead lead. It's someone who raised their hand and got busy. My system is 8 follow-ups, each one a real email with something new in it. Eight fresh reasons to book, not eight bumps.

The three laws:

1. **Every touch brings a NEW angle.** Never "just bumping this", never "circling back". A follow-up with no new idea is a nag, and nags train people to ignore the thread.
2. **Every touch names specific proof with a number.** "I rebuilt one client's checkout flow and mobile conversion went from 1.1% to 2.4% in five weeks" beats "we've helped companies improve conversion" every time. All eight touches carry a named case study with an exact number, never the same one twice in a row.
3. **Every touch ends on a varied CTA around the clickable calendar link.** The link appears every time; the sentence around it changes every time.

The mechanics: minimum 3 days between touches (roughly days 3, 6, 10, 14, 18, 22, 26, 30, widening as it ages), sent as thread replies on the same conversation, 150 words max (the final soft close is 50-70). And the one technical detail people get wrong: **these sends are HTML, not plain text.** Cold sends are plain text; follow-up replies must stay HTML or the calendar link flattens into a dead line and the whole system is pointless. In EmailBison terms: `reply_followup` campaigns always run `plain_text: false`.

Stopping rules: after touch 8, stop (the soft close only works if it's actually last). Any "no" stops it that second. Unsubscribe removes them everywhere. Any reply at all exits the sequence immediately and goes back to live conversation.

---

## What to do next

Wire the loop before launch day: know your buckets, have your reply shapes ready, and have the 8 follow-ups drafted in advance so a stalling thread never waits on you. The full skills: `/reply-classification`, `/reply-drafting`, `/reply-followups`.

If your interested rate is sitting under 5%, stop polishing replies. The problem is upstream: `/offer-framing`.
