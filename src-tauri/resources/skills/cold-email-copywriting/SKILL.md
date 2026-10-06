---
name: cold-email-copywriting
description: Writes cold email copy using Joel's two base frameworks (Offer First and Built For You) with poke and hook opener layers stacked on top, plus the full house style and hard bans. Use whenever writing or revising cold email copy of any kind. Triggers on "write my copy", "write cold emails", "new variant", "rewrite this email", "draft a campaign", "copy for this offer", "subject lines", "review my copy", "clean up this email", "the copy isn't working", "give me variants", "write the first email", or any request to produce, fix, or vet outbound email copy. Takes an offer and audience as input and outputs finished variants ready for sequencing and spintax.
---

# Cold Email Copywriting

This is my copy system. It runs every campaign I send, across every client account
at Pipeline Playbooks. It does one job: take an offer and an audience as input,
then frame and write that offer as cold email copy across a small set of proven
structures.

What it does NOT do: invent offers, pick targeting, or build the list. Those are
upstream. If you don't have a clear offer yet, stop and run `/offer-framing`
first. Copy exists to test an offer. It cannot rescue a bad one.

## When to use this

- Writing a new campaign from scratch
- Writing new variants for an existing campaign
- Rewriting copy that isn't pulling replies
- Vetting copy someone else wrote before it goes anywhere near a send tool
- Writing subject lines

## The three layers (keep them separate)

Every email is the product of three decisions, made in this order:

| Layer | Question | Where it's decided |
|---|---|---|
| 1. Offer | WHAT are we putting in front of them? | Input to this skill (`/offer-framing`) |
| 2. Frame | What do they read first, and in what structure? | This skill: the two bases + layers |
| 3. Execution | How does it sound, look, read? | This skill: house style + bans |

Hold the offer constant, vary the frame and execution. The same offer written
through every structure produces a genuinely different email each time. That is
the whole game: you are testing the offer, and the copy's job is to not get in
the way.

---

## The two BASE frameworks

Every cold email I send is built on one of two bases. Not three, not five. Two.

### Base 1: Offer First

The offer is the first thing they read after their name. No warm-up, no
throat-clearing. You state the thing, you ask if it's useful.

Skeleton:

```
{{FIRST_NAME}}, [the offer, framed as a value question or a direct statement].

[Optional: one credibility line.]

[Interest-check CTA.]

{{SENDER_FIRST_NAME}}
[Company name]
```

The offer line is framed as a **value question**, never a favour:

- "If I put together [deliverable] for {{COMPANY}}, would that be useful?"
- "Would it be valuable if I [deliverable] for {{COMPANY}}?"
- "Got an idea for {{COMPANY}}: [deliverable]. Would that be useful?"

Never "Happy to send it over" or "We'd love to show you". Those give away power.
The reader should be evaluating whether the thing is valuable to them, not
deciding whether to accept your generosity.

A self-evidently valuable offer often converts BEST completely naked: offer, CTA,
done. Don't assume an email needs more just because you have more material. The
"3 ideas" email and the "we just released X" email are both Offer First emails;
they're offer content, not separate frameworks.

### Base 2: Built For You

You already made the thing. Past tense. The email announces a specific asset that
exists for THEM, then asks if they want it.

Skeleton:

```
{{FIRST_NAME}}, just built {{COMPANY}} [specific asset]: [what it is in their
language, one line].

[The payoff: what having it changes, ideally with a number.]

Want me to send it over?

{{SENDER_FIRST_NAME}}
[Company name]
```

Rules that make Built For You work:

- **Past tense.** "Just built", "put together", "made". Not "I could build" or
  "I'd be happy to create". The asset exists. That's the hook.
- **Name the asset concretely.** "A sample dispatch playbook", "a 6-email
  post-purchase flow", "a teardown of your category's pricing pages". Vague
  assets ("some ideas about your marketing") kill this frame.
- **Payoff, then interest check.** One line on what it changes, then "want me to
  send it over?"
- **Be honest about scale.** Only use Built For You when you can genuinely
  produce the asset for everyone who says yes. If the asset needs bespoke work
  per prospect, either template it so it IS producible at volume, or switch to
  Offer First and tease it conditionally ("if I put together X for you, would
  that be useful?") and build it after they reply. Never claim finished bespoke
  work that doesn't exist.

### The bespoke-claims rule (applies to both bases)

Cold email goes out at volume, so the copy cannot depend on custom research per
prospect. Never claim "we looked at YOUR site and found these three issues"
unless that check is automated and genuinely runs on every domain in the list.
If the finding needs human work per prospect, tease it as an offer-to-do
instead. The first email's claim must be true for the entire list.

---

## The layers: poke and hook

Poke-the-bear and Hook are NOT frameworks. They are **opener layers** you stack
on top of either base. A layer changes what line one is; the base underneath
stays the same. This matters because it makes them single-variable tests: same
base, same offer, different opener, and now you know exactly what moved the
number.

### Poke layer (a question)

Line one becomes a curiosity question that surfaces a pain or makes them picture
the better state. Then the base email follows.

Opener styles to rotate:

- Conditional: "If [improved state], would that change [metric they care about]?"
- Diagnostic: "Quick question: [question that qualifies their situation]?"
- Status-quo naming: "Where does {{COMPANY}}'s team go when they need to [action]?"
- Tool callout: "Still running [pain point] through [tool1] or [tool2]?"
- Intent qualifier: "Is [initiative] on the radar for {{COMPANY}} this year?"

The question must earn the next line. If the honest answer is "no, not relevant",
the email dies right there. Write questions where the honest answer makes them
lean forward.

### Hook layer (a statement)

Line one becomes a punchy declaration: an uncomfortable truth, a pattern
observation, a contrarian claim. Not a question.

Hook styles to rotate:

- Pain naming: "Most [ICP type] [common failure] because [root cause]."
- Contrarian: "[Common belief] is wrong. The real issue is [reframe]."
- Pattern: "The [ICP type] with the best [outcome] have one thing in common: [insight]."
- Status-quo attack: "[Common approach] has a problem: [specific consequence]."

The hook should describe their problem better than they could themselves. If it
reads like a LinkedIn post opener, cut it and write a plainer one.

### How a variant set is built

A variant set for one offer = the two bases, plus poke-layered and hook-layered
versions of each, with block-level variations (credibility line in or out, ps
line in or out, CTA wording rotated). That's how 10-12 variants per base stay
genuinely different instead of reading copy-pasted. Every variant against a
control changes ONE variable. Never assume poke beats hook or naked beats full;
which wins is a test result, not a preference.

---

## The credibility block

When you add proof, use this exact shape (in the body or the ps line):

> "We've helped [X number] of [specific persona] achieve [outcome in their
> language] without [the pain they'd expect]."

Example: "We've helped 60+ freight brokerages cut quote turnaround from days to
hours without changing their TMS."

The "without [pain]" clause is the part everyone skips and shouldn't: it's what
turns a bare stat into a credibility statement. Name the persona specifically
(not "companies", not "clients") and the outcome in their words (not "results").

Proof rules regardless of placement:

- **Exact numbers, always.** "42% more monthly active users" beats "great
  results". "From 19 days to 6" beats "much faster". If you don't have a real
  number, don't fake one; write the email without proof instead.
- **Rotate proof points across the set.** Never the same stat in every variant.
- **Name the ICP, not the customer** unless the name is cleared for public use.
  "A mid-size 3PL" works fine.
- **Explain the why in some variants.** "3x signups because users never left the
  product" is more believable than "3x signups".

---

## House style (every email, no exceptions)

1. **First name inline in the first sentence.** Never "Hey {{FIRST_NAME}}," as a
   greeting line. The name opens the first sentence and the sentence keeps
   going: "{{FIRST_NAME}}, quick question about..." It reads like a colleague,
   not a mail merge.
2. **The value question IS the CTA.** You don't pitch and then ask. The ask and
   the value live in one question: "If I sent over the playbook, would that be
   useful?" One job per email.
3. **50-90 words.** Count them. Under 50 is fine if the offer carries it. Over
   90 means something isn't earning its place.
4. **One sentence per paragraph.** Line break after every sentence. No walls of
   text. Cold emails get read on phones.
5. **Exact metrics, never vague claims.** See the proof rules above.
6. **Subject lines lowercase, 2-4 words.** More below.
7. **`ps:` lowercase** when you use a trailing line. The ps is often the
   most-read line in the email; it's the natural home for a second proof point.
8. **Sign-off: sender first name, company name on the next line.** No "Kind
   regards", no "Warmest wishes".
9. **Tone: laid back, curious, confident, punchy.** A smart human, not a brand.
10. **Cut filler.** If a word can go, it goes.

## Hard bans (breaking these is a rewrite, not a note)

- **NO links, email addresses, or domains anywhere in cold copy.** Not in the
  body, not in the signature, not in the ps. URLs are the single biggest spam
  trigger in a cold first touch. If you absolutely must reference something that
  is normally written as a domain, write it with dashes the way you'd write a
  phone number, so nothing in the email pattern-matches as a URL. Better: don't
  reference it at all and hand it over after they reply.
- **NEVER mention calls, meetings, or time commitments.** No "15 minutes", no
  "quick chat", no "hop on a call", no "next Tuesday". Cold email sells the
  reply, nothing else. Time asks come later, in the reply thread, softened.
- **CTAs are pure interest checks.** "Want me to send it over?", "Would that be
  useful?", "Reply 'yes' and I'll send it across." The prospect should be able
  to answer in one word. Any CTA that asks them to do work, commit time, or
  take a next step is banned.
- **No em dashes.** Use commas, colons, or a full stop and a new sentence.
- **Banned CTAs:** "Worth a look?", "Worth a chat?", "Worth exploring?",
  "Interested?", "Open to it?", any "worth a [noun]?" construction. Use reply
  triggers ("Reply 'yes' if that sounds useful") or value questions ("Would
  that be useful?") instead.
- **Banned framing:** "Happy to [X]", "We'd love to [X]", "I'd love to show
  you". Frame as a value question instead.
- **Banned words:** synergy, leverage, cutting-edge, game-changer, best-in-class,
  transformative, utilize, very, really, basically, literally, plus the usual
  corporate sludge. If it sounds like a press release, delete it.
- **Spam-word scan before anything ships.** The #1 offender is **"free"**: write
  "on us", "no charge", or "on the house" instead, every time. Also banned:
  "guaranteed", "limited time", "act now", "click here", "risk-free",
  "no obligation", "exclusive deal", multiple exclamation marks, ALL-CAPS words.
  Re-run the scan after EVERY edit pass, including edits a client or teammate
  hands back. Someone asking for "free" in the copy doesn't override the list.

## Variables

Double curly braces, ALL CAPS, underscores for spaces:

- `{{FIRST_NAME}}` : prospect first name
- `{{COMPANY}}` : prospect company name
- `{{SENDER_FIRST_NAME}}` : sender name in the signature

Everything else is written copy. No bespoke per-prospect variables in a first
touch unless the data genuinely exists for the whole list. Never force a
personalization token in just to have one; beyond first name and company there
often isn't one worth using.

## Subject lines

One job: get the open. It is not an ad headline. It should look like something
a colleague or a customer would send.

- **2-4 words, all lowercase.** Never all caps, no emojis, no exclamation
  points, no ellipsis.
- **What works, in order of reliability:**
  1. `{{COMPANY}}` on its own. Impossible to ignore.
  2. `quick question`. Hated by email Twitter, still wins tests. Always run it
     as a baseline.
  3. `intro`. Same story.
  4. A 2-3 word product or industry reference: `dispatch playbooks`,
     `quote turnaround`. Reads like an internal note.
  5. `{{COMPANY}}` + context word: `{{COMPANY}} ops`.
- **Avoid:** anything over 5 words, value propositions in the subject, questions
  in the subject (save them for the body), blog-post energy, and the word
  "free" anywhere near a subject line.
- **Always A/B a creative subject against `quick question` or `intro`.** The
  boring baseline wins more often than anyone expects.

---

## The process (what I actually do, in order)

1. **Confirm the inputs.** Offer (deliverable, who it's for, outcome, proof
   points), persona (role, pains, their language), sender name. Missing an
   offer? Stop, run `/offer-framing`.
2. **Classify the offer.** Fast and self-evident (audit, teardown, sample
   asset, "3 ideas")? It compresses to one line and Offer First naked will be
   strong. Strategic (playbook, system, framework)? It may need a poke or hook
   layer to build runway first.
3. **Write the Offer First set.** Naked first, then + credibility, then + ps
   variants. Rotate the offer-line phrasing and the CTA wording so no two open
   or close the same way.
4. **Write the Built For You set.** Same block rotation.
5. **Layer poke and hook versions of each base.** Rotate opener styles. Each
   layered variant differs from its base by the opener ONLY, so it tests clean.
6. **Write subject lines.** Baselines plus 2-3 candidates per the rules above.
7. **Run the kill pass.** Word count 50-90. No links, no domains, no email
   addresses. No calls, meetings, or time words. No em dashes. No banned words,
   CTAs, or framing. Spam scan including "free". Name inline in sentence one.
   Exact numbers only. Variables formatted `{{LIKE_THIS}}`.
8. **Run the diversity check** (below). Then output pure copy: every line on
   its own line, one blank line between variants, no commentary.

### Variant diversity checklist

Before a set ships:

- [ ] At least 2 variants per base are naked (minimal blocks, under 50 words)
- [ ] At least 2 variants per base carry a credibility line
- [ ] At least 2 variants per base carry a ps line
- [ ] No two variants use the same proof point in the same position
- [ ] No more than 2 variants share a CTA wording
- [ ] No two variants open the offer the same way
- [ ] Word counts range from ~40 to ~90 across the set

---

## Worked examples (fictional company throughout)

Scenario: the sender is **Freightline Labs**, a fictional ops consultancy that
helps mid-size freight brokerages speed up quoting and dispatch. The offer is a
same-day quoting playbook. All names and numbers below are invented for
illustration.

### Offer First, naked (Shape: offer, CTA)

Subject: `{{COMPANY}}`

```
{{FIRST_NAME}}, I put together a breakdown of how three mid-size brokerages cut
quote turnaround from 4 days to same-day without adding ops headcount.

It covers the exact workflow changes, in order, with the numbers at each step.

Want me to send it over?

{{SENDER_FIRST_NAME}}
Freightline Labs
```

~52 words. Offer, one line of substance, interest check. Nothing else needed.

### Offer First, + credibility (value-question form)

Subject: `quote turnaround`

```
{{FIRST_NAME}}, if I put together a same-day quoting playbook for {{COMPANY}},
would that be useful?

We've helped 60+ freight brokerages cut quote turnaround from days to hours
without changing their TMS.

Reply "yes" and I'll send it across.

{{SENDER_FIRST_NAME}}
Freightline Labs
```

The value question is the CTA's setup and the credibility line does the trust
work. Note the "without changing their TMS" clause: that's the formula.

### Offer First, + ps

Subject: `quick question`

```
{{FIRST_NAME}}, got an idea for {{COMPANY}}: a one-page quoting checklist your
team runs before any quote goes out, built from what the fastest brokerages in
your lane size do differently.

Would that be useful?

{{SENDER_FIRST_NAME}}
Freightline Labs

ps: one team using this checklist went from 31% to 54% quote-to-book in a
quarter.
```

Body stays fast; the proof sits in the most-read line.

### Built For You

Subject: `{{COMPANY}}`

```
{{FIRST_NAME}}, just built {{COMPANY}} a sample dispatch playbook: role-based,
one page per scenario, the thing your ops team pulls up the second a load goes
sideways instead of digging through email threads.

One brokerage running this format cut check-in calls by half in six weeks.

Want me to send it over?

{{SENDER_FIRST_NAME}}
Freightline Labs
```

Past tense, concrete asset, payoff with a number, one-word-answer CTA.

### Poke layer on the Offer First base

Subject: `quick question`

```
{{FIRST_NAME}}, if every quote left {{COMPANY}} the same day it came in, would
that change your win rate?

I put together a playbook showing exactly how three brokerages got there
without new hires.

Would it be useful if I sent it over?

{{SENDER_FIRST_NAME}}
Freightline Labs
```

Same base email as the naked variant, opener swapped in. One variable changed.

### Hook layer on the Built For You base

Subject: `dispatch playbooks`

```
{{FIRST_NAME}}, most brokerages lose loads to slow answers, not bad pricing.

So we built {{COMPANY}} a sample dispatch playbook: one page per scenario, the
thing your team checks before they start phoning around.

Want me to walk you through it?

{{SENDER_FIRST_NAME}}
Freightline Labs
```

The hook names the uncomfortable truth; the base underneath is unchanged.

### What a FAILING email looks like (don't ship this)

```
Hey {{FIRST_NAME}},

Hope this finds you well! I'd love to hop on a quick 15-minute call to show you
our cutting-edge platform. We help companies like yours leverage automation to
drive results. Check out our website at freightlinelabs dot com for a free
demo!

Worth a chat?
```

Everything wrong at once: greeting line instead of name inline, time ask, banned
words (love to, cutting-edge, leverage), vague claims, a domain reference, the
word "free", and a banned CTA. This email gets deleted and hurts the domain
sending it.

---

## Benchmarks (what "working" means)

Judge copy on **interested reply rate**, not opens:

- **20%+ of replies interested: great.** Scale it.
- **10%+: good.** Keep testing angles against it.
- **Under 5%: bad.** The offer or the list is wrong; more copy variants won't
  save it.
- **0% with real volume behind it: pause and rework the offer.**

If interested replies are low across every variant, the problem is almost never
execution. It's the offer or the audience. Go back upstream before writing
variant number thirteen.

---

## Common mistakes

1. **Writing three siloed template families instead of two bases + layers.**
   Then every test changes multiple variables and you learn nothing.
2. **"Hey {{FIRST_NAME}}," on its own line.** Reads like a mail merge. Name
   inline, sentence keeps moving.
3. **Pitching, then asking.** The value question IS the CTA. Two asks in one
   email is zero asks.
4. **Vague proof.** "Companies see great results" is worse than no proof line.
5. **Sneaking in a time ask.** "Just 10 minutes" is still a time ask. The only
   thing a cold email sells is the reply.
6. **A link "just in the signature".** No. No links anywhere.
7. **Claiming bespoke work you didn't do.** If the audit doesn't run on every
   domain automatically, tease it conditionally instead.
8. **Fixing weak offers with longer emails.** Length is not leverage. If the
   naked version doesn't work, the fix is upstream.
9. **Shipping without the spam scan, or skipping the re-scan after edits.**
   "free" creeps back in through client edits constantly.
10. **Same CTA on every variant.** Rotate reply triggers and value questions or
    the set pattern-matches as identical.

## What to do next

Run `/sequence-architecture`. Your variants are single emails; that skill places
them into the 4-step structure (which steps thread, which start fresh, and the
timing between them) so the campaign is an actual sequence and not one email
fired four times.

## Related skills

- `/offer-framing` : decides WHAT the email says; run it before this skill if
  the offer isn't nailed down
- `/lead-magnet-builder` : builds the asset a Built For You email announces
- `/offer-testing-roadmap` : plans which angle each campaign tests, one per week
- `/sequence-architecture` : turns these variants into the 4-step sequence
- `/spintax-depth` : adds send-level variation once a variant has volume
- `/pre-launch-qa` : the final gate before anything is activated
