# 03: Copywriting (condensed)

The condensed house rules plus one complete worked example. The full system, with variant-set construction, the poke and hook opener layers, subject line doctrine, and a dozen more examples, lives in `/cold-email-copywriting`. Copy exists to test an offer; if the offer isn't nailed down, go back to `02-offers-and-strategy.md` first.

## The three layers

Every email is three decisions, made in order:

1. **Offer:** WHAT goes in front of them. Decided upstream, in `/offer-framing`.
2. **Frame:** what they read first and in what structure. The two bases below.
3. **Execution:** how it sounds. The house style and the bans.

Hold the offer constant, vary the frame and execution. You're testing the offer; the copy's job is to not get in the way.

## The two bases

**Offer First.** The offer is the first thing they read after their name. No warm-up, no throat-clearing. The offer line is framed as a value question, never a favour: "If I put together {deliverable} for {{COMPANY}}, would that be useful?" Never "happy to send it over"; the reader should be evaluating whether the thing is valuable, not accepting your generosity. A self-evidently valuable offer often converts best completely naked: offer, CTA, done.

**Built For You.** You already made the thing. Past tense ("just built", not "I could build"), a concretely named asset, one payoff line ideally with a number, then "want me to send it over?" Only use it when you can genuinely produce the asset for everyone who says yes; never claim finished bespoke work that doesn't exist. If the finding needs human work per prospect, tease it conditionally with Offer First instead. The first email's claim must be true for the entire list.

On top of either base you can layer a **poke** opener (a curiosity question that surfaces the pain) or a **hook** opener (a punchy statement that names it). Layers change line one only; the base stays the same, so every test moves a single variable.

## House style (every email, no exceptions)

1. **First name inline in the first sentence**, never "Hey {{FIRST_NAME}}," as a greeting line. "{{FIRST_NAME}}, quick question about..." reads like a colleague, not a mail merge.
2. **The value question IS the CTA.** You don't pitch and then ask; the ask and the value live in one question. One job per email.
3. **50-90 words.** Count them. Under 50 is fine if the offer carries it; over 90 means something isn't earning its place.
4. **One sentence per paragraph.** Line break after every sentence. Cold emails get read on phones.
5. **Exact metrics, never vague claims.** "From 19 days to 6" beats "much faster". No real number? Write the email without proof rather than faking one.
6. **Subject lines lowercase, 2-4 words.** `{{COMPANY}}` alone, `quick question`, and `intro` are the boring baselines that keep winning tests. Always A/B a creative subject against one of them.
7. **`ps:` lowercase** when you use a trailing line. It's often the most-read line in the email and the natural home for a second proof point.
8. **Sign-off: sender first name, company name on the next line.** No "kind regards".
9. **Tone: laid back, curious, confident, punchy.** A smart human, not a brand.

When you add proof, use the credibility shape: "We've helped {X number} of {specific persona} achieve {outcome in their language} without {the pain they'd expect}." The "without" clause is the part everyone skips and shouldn't.

## Hard bans (breaking one is a rewrite, not a note)

- **No links, email addresses, or domains anywhere in cold copy.** Not the body, not the signature, not the ps. URLs are the biggest spam trigger in a first touch.
- **Never mention calls, meetings, or time.** No "15 minutes", no "quick chat". Cold email sells the reply, nothing else; time asks come later, in the reply thread.
- **CTAs are pure interest checks**, answerable in one word: "Want me to send it over?", "Would that be useful?" Banned: "worth a chat?", "interested?", "open to it?", any CTA asking them to do work or commit time.
- **No em dashes.** Commas, colons, or a new sentence.
- **Spam-word scan before anything ships**, and again after EVERY edit pass. The number one offender is "free": write "on us" or "on the house". Also banned: guaranteed, limited time, act now, click here, risk-free, plus the corporate sludge (leverage, synergy, cutting-edge, game-changer).
- **Variables are `{{DOUBLE_CURLY_UPPERCASE}}`**: `{{FIRST_NAME}}`, `{{COMPANY}}`, `{{SENDER_FIRST_NAME}}`. Beyond name and company there's rarely a personalization token worth forcing in.

## One complete worked example

Fictional throughout: **Ledgerline**, a payroll service for mid-size construction firms. The offer (from the `/offer-framing` decision stack): a free "payroll leak check", a reveal-the-problem deliverable showing where a contractor's payroll is overpaying on labor codes. Proof point (invented): found a 90-person groundworks firm $40k/yr in miscoded overtime.

Subject: `payroll codes`

```
{{FIRST_NAME}}, if I ran a payroll leak check for {{COMPANY}} and sent over
what we find, no charge, would that be useful?

We've helped 30+ mid-size contractors claw back five figures a year in
miscoded overtime without switching payroll systems.

Reply "yes" and I'll get it moving.

{{SENDER_FIRST_NAME}}
Ledgerline

ps: the last one we ran found a 90-person groundworks firm $40k a year.
```

Why this passes the kill pass:

- 62 words. Name inline, sentence keeps moving.
- The value question is the CTA; the reply trigger just makes answering effortless.
- Credibility line follows the formula, persona named ("mid-size contractors"), exact numbers, and a "without" clause that kills the obvious objection (switching systems).
- Concrete named deliverable ("payroll leak check"), not "a free audit" or "a consultation".
- "No charge", never "free". No links, no time ask, no em dashes, lowercase 2-word subject, lowercase ps carrying the second proof point.

And the same email failing, so you recognize the shape:

```
Hey {{FIRST_NAME}},

Hope you're well! I'd love to hop on a quick 15-minute call to show you how
we leverage cutting-edge payroll tech to drive results for companies like
yours. Check out ledgerline dot com for a free audit!

Worth a chat?
```

Greeting line, time ask, banned framing and words, vague claims, a domain reference, "free", and a banned CTA. Delete on sight.

## The kill pass (run before anything ships)

Word count 50-90. No links, domains, or email addresses. No calls, meetings, or time words. No em dashes. No banned words, CTAs, or framing. Spam scan including "free". Name inline in sentence one. Exact numbers only. Variables formatted `{{LIKE_THIS}}`. Re-run after every edit, including edits someone else hands back.

## Benchmarks

Judge copy on **interested reply rate**, not opens: 20%+ of replies interested is great, 10%+ is good, under 5% is bad. If interested replies are low across every variant, the problem is almost never execution; it's the offer or the audience. Go upstream before writing variant thirteen.

## Next

`/cold-email-copywriting` for the full system and variant construction. Then `04-sequence-structure.md`: single emails become a campaign.
