---
name: spintax-depth
description: Adds spintax variation to cold email copy and decides how deep to spin, light at launch, deep once a variant passes roughly 2,000 cumulative sends. Use whenever spinning copy, adding variation, or diagnosing pattern-matching. Triggers on "spintax this", "spin this email", "add variation", "add spintax", "deepen the spintax", "every send looks the same", "this variant has a lot of volume on it", "spam filters are pattern-matching us", "spin the CTA", or any request to vary copy at the send level without changing what it says. Also covers what must NEVER be spun: numbers, names, merge tags, and the offer's unique mechanism.
---

# Spintax Depth

Spintax makes every send of the same variant read slightly differently, so a
thousand copies of one email don't look like a thousand copies of one email.
It is deliverability insurance, not creativity: the message never changes, only
the wording around it.

The stakes are real. A broken spin combination, a drifted number, or a mangled
sentence goes out to real prospects under your name. When unsure whether
something is safe to spin, leave it alone.

## When to use this

- Spinning finished copy before it goes into a campaign
- Deepening the spin on a variant that has accumulated real volume
- Reviewing existing spintax for broken combinations
- Deciding whether a variant needs deeper spin yet

Copy comes first. Spin finished, approved variants from
`/cold-email-copywriting`; never draft and spin in the same pass.

## Syntax

- Spin groups use single curly braces with pipes: `{option one|option two|option three}`
- Merge tags use double curly braces, all caps: `{{FIRST_NAME}}`, `{{COMPANY}}`,
  `{{SENDER_FIRST_NAME}}`
- The two must stay visually distinct. Never put a merge tag's braces inside a
  spin decision, and never spin the tag itself.
- Don't nest spin groups. Flat groups only.

## The core principle

Spin the parts that carry **no unique meaning**: the words a human would
naturally phrase a dozen ways without changing what's being said. Protect
anything that is a fact, a name, a number, or the one specific idea that makes
the email land.

The test for any phrase: *"If I swap this for a synonym, does the meaning, the
proof, or the specificity change at all?"* No: safe to spin. Yes: protect it.

## The one non-negotiable: every path must read naturally

Spintax is only worthwhile if **every possible combination** of the options
still reads as a natural, grammatical email a real person would send. Before
returning spun copy, mentally draw several random combinations and read them
straight through. If any combination is awkward, ungrammatical, repetitive, or
confusing, fix the bank. One broken path in a 10-option block means roughly
10% of your sends go out broken.

What "reads naturally in every path" requires:

1. **Mutual substitutability.** Every option in a block must slot cleanly into
   the exact surrounding words: same tense, same number, no double-ups with
   neighbouring text. `{Got|Put together|Came up with} a few ideas` works;
   `{Got|I have got|There are} a few ideas` breaks two of three paths against a
   sentence that already has a subject.
2. **Roughly even option lengths.** Don't pair a 3-word option with a 15-word
   option in one block. Wildly uneven options change the email's shape per send
   and read worse in the short paths.
3. **Tone preservation.** Every option must sit in the same register as the
   line it replaces. If the email is casual British, no option gets to be
   formal American. The voice must survive every path; spin that drifts the
   voice is worse than no spin.
4. **No meaning drift.** You are rephrasing one idea many ways, never saying a
   new thing. No new claims, no softened claims, no dropped clauses.

## Never spin (protect exactly as written)

- **Merge tags**: `{{FIRST_NAME}}`, `{{COMPANY}}`, `{{SENDER_FIRST_NAME}}`
- **Numbers and metrics.** The value NEVER changes. "4 days to same-day" can
  render as "~4 days" but never becomes "5 days". Metric drift is the single
  most damaging spin error; guard against it harder than anything else.
- **Brand, product, and company names**, including the prospect's
- **Named tools and technologies**, exact spelling and capitalisation
- **Case studies**: a persona and its result stay locked together as one unit
- **The unique mechanism**: the one specific idea that makes the offer
  distinctive (a pricing model, a method, a specific deliverable). Spin the
  wording around it; the mechanism itself survives intact in every path.
- **Literal reply words.** If the CTA says reply "yes", the word "yes" stays
  exactly that in every option, even while the sentence around it spins.
- **Acronyms, titles, and proper nouns**

## Spin density by section

Not every line gets equal treatment:

| Section | Density |
|---|---|
| CTA line | Heaviest: 10-30+ options where natural |
| Sign-off | Heavy: `{Thanks|Cheers|Best|Speak soon}` |
| Opt-out line | Heavy: full bank, present in every email |
| Body wording | Free: multiple spin points per line, word/phrase/clause level |
| Fact-carrying sentences | Zero |

The logic: housekeeping lines (CTA, sign-off, opt-out) are short and
interchangeable, so they tolerate huge banks and they're the lines every email
shares, which makes them the biggest pattern-match risk. The body carries the
message, so it gets spun freely at the wording level but never at the meaning
level. Fact lines get nothing.

Two standing rules I apply to every spun email:

- **Every email carries an opt-out line**, heavily spun ("if this isn't
  relevant, just reply 'no thanks'" and variants). If the pasted copy is
  missing one, it goes in anyway. No email ships without it.
- **Blank line after every line.** Greeting-through-ps, double-spaced, so the
  spacing survives the paste into the sending tool.

## When to deepen (the 2,000-send threshold)

Depth is staged, not maxed from day one:

- **At launch: light spin is fine.** The CTA bank and opt-out bank are always
  in (non-negotiable), but the body doesn't need deep spin while a variant is
  new and low-volume. You don't yet know if this variant deserves the
  investment.
- **Past roughly 2,000 CUMULATIVE sends on a variant: deepen the body.** More
  spin points per line, bigger banks per point, phrase- and clause-level swaps,
  not just single words. By this volume the variant has proven it's staying in
  rotation, and enough copies exist in the wild that pattern-matching becomes a
  real risk.
- **Merged variants pool their send counts.** If you merged two near-identical
  variants, the merged variant's cumulative total is the SUM of what it
  absorbed, and that sum is what you compare against the ~2,000 threshold. Not
  the count since the merge.
- Deepening may also add personalization tokens, but ONLY genuinely relevant
  ones. Beyond first name and company there often are none. Never force a
  token in just to have one.

## Worked example: shallow vs deep (fictional company)

The base email, from Freightline Labs (fictional ops consultancy for freight
brokerages):

```
{{FIRST_NAME}}, I put together a breakdown of how three mid-size brokerages cut
quote turnaround from 4 days to same-day without adding ops headcount.

Want me to send it over?

{{SENDER_FIRST_NAME}}
Freightline Labs
```

### Shallow spin (launch depth)

Opt-out in, CTA bank in, one light touch on the opener. Fact line untouched.

```
{{FIRST_NAME}}, I {put together|pulled together} a breakdown of how three
mid-size brokerages cut quote turnaround from 4 days to same-day without adding
ops headcount.

{Want me to send it over?|Want me to send it across?|Would it be useful if I
sent it over?|Shall I send it your way?|Reply "yes" and I'll send it over|Want
a copy?|Would that be useful to see?|Mind if I send it through?|Want me to
share it?|Want me to run it by you?}

{Cheers|Thanks|Best},

{{SENDER_FIRST_NAME}}

Freightline Labs

{if this isn't relevant, just reply "no thanks"|not for you? just reply "no
thanks"|a quick "no" and you won't hear from me again|if you'd rather not get
these, just say "no thanks"|want me to stop? just reply "no thanks"}
```

### Deep spin (past ~2,000 cumulative sends)

Multiple spin points per body line, phrase-level swaps, bigger banks. The
protected material is IDENTICAL in every path: "three", "mid-size brokerages",
"4 days to same-day", "ops headcount" as the pain clause's meaning, and both
merge tags.

```
{{FIRST_NAME}}, I {put together|pulled together|wrote up|built} a
{breakdown|walkthrough|short breakdown} of how three mid-size brokerages
{cut|took} quote turnaround from 4 days to same-day without
{adding ops headcount|hiring more ops people|growing the ops team}.

{It covers|It walks through|Inside is} {the exact workflow changes, in
order|each workflow change, step by step|the exact changes they made, in
sequence}, {with the numbers at each step|with the before-and-after numbers|
numbers included at every step}.

{Want me to send it over?|Want me to send it across?|Would it be useful if I
sent it over?|Shall I send it your way?|Reply "yes" and I'll send it over|Reply
"yes" if that sounds useful|Want a copy?|Would that be useful to see?|Mind if
I send it through?|Want me to share it?|Want me to run it by you?|Curious to
see it?|Want me to fire it over?|Say the word and I'll send it|Reply "sure"
and I'll send it across|Would you want to see it?|Mind if I share it?|Want it
in your inbox?|Shall I share it?}

{Cheers|Thanks|Best|All the best|Speak soon},

{{SENDER_FIRST_NAME}}

Freightline Labs

{if this isn't relevant, just say "no thanks"|not for you? just reply "no
thanks"|a quick "no" and you won't hear from me again|if you'd rather not get
these, just say "no thanks"|want me to stop? just reply "no thanks"|if this
misses the mark, just say "no thanks"|rather not hear from me? just say "no
thanks"|not interested? just reply "no thanks"|if you'd prefer I didn't email,
just say "no thanks"|not the right time? just say "no thanks"}
```

Read a few random paths of the deep version aloud. Every one is a natural
email; every one says exactly the same true things; no path changed "4 days"
or "three brokerages" or weakened "without adding ops headcount" into
something vaguer. That's the standard.

### What a BROKEN spin looks like (never ship)

```
I put together a breakdown of how {three|several|a few} brokerages cut quote
turnaround from {4 days|around a week} to same-day...
```

Two protected values spun: the count and the metric. Now some sends make a
claim that isn't true. This is the exact failure mode the never-spin list
exists to prevent.

## Self-check before returning spun copy

- [ ] Several random combinations read through, all natural and grammatical
- [ ] Every merge tag, name, number, case study, and the unique mechanism
      untouched in every path
- [ ] No number's value changed anywhere
- [ ] Option lengths roughly even within each block
- [ ] Register consistent with the original in every option
- [ ] CTA spun to 10-30+ options where natural
- [ ] Opt-out line present and spun
- [ ] Blank line after every line
- [ ] Nothing added, nothing removed: rephrased in place only

## Common mistakes

1. **Spinning the proof.** "Cut turnaround {significantly|by a lot|from 4 days
   to same-day}" destroys the one line doing the work, and two of three paths
   now have no number.
2. **One awkward option in a big bank.** It still ships in a share of your
   sends. Every option earns its place or gets cut.
3. **Grammar breaks at the seams.** Options tested in isolation, not against
   the surrounding sentence.
4. **Deep-spinning everything on day one.** Wasted effort on variants that may
   die at 300 sends, and more surface area for broken paths. Stage it.
5. **Forgetting the pooled counts after a merge.** A merged variant can cross
   2,000 the day it's created.
6. **Nesting spin groups.** Hard to audit, easy to break, no upside.
7. **Spinning the literal reply word.** If the email says reply "yes", every
   path says "yes".
8. **Register drift.** A formal option in a casual email is a tell in every
   send it lands in.
9. **Treating spintax as a rescue for tired copy.** If a variant's reply rate
   is falling, deeper spin isn't the fix; a new angle is. Spin protects
   deliverability, it doesn't create interest.

## What to do next

Run `/pre-launch-qa`. Spun copy is the last content step; the QA gate checks
the campaign settings, senders, schedule, and the copy itself (including a spam
scan of every path-visible word) before anything is activated.

## Related skills

- `/cold-email-copywriting` : writes the variants this skill spins; spin
  approved copy, never drafts
- `/sequence-architecture` : the step structure the spun copy slots into
- `/pre-launch-qa` : the gate before launch; re-check spin after ANY copy edit
- `/emailbison-campaign-launch` : uploads the spun sequence, draft only
