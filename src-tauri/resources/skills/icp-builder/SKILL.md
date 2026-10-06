---
name: icp-builder
description: Conversational intake that produces profiles/profile.yaml, the shared file every other skill in this repo reads. Scrapes the user's website first, then interviews them one question at a time about their company, offer, pricing model, risk reversals, ICP (industries, titles, company size, geography), proof points, case studies, and calendar link. Use at the very start of any cold email project, before building lists or writing copy. Triggers on "set up my ICP", "build my profile", "onboard me", "who should I be targeting", "define my ideal customer", "new campaign for my business", "start a cold email campaign", or whenever another skill needs profile.yaml and it does not exist yet.
---

# ICP Builder

This skill produces one file: `profiles/profile.yaml`. Every other skill in this repo reads it. The list builder pulls its filters from it, the offer skills pull the pricing model and proof from it, the copywriting skill pulls the tone and case studies from it. Get this file right and everything downstream gets easier. Get it wrong and you will be sending good emails to the wrong people.

I run this same intake for every client at my agency. The difference between "B2B SaaS founders" and "Head of Talent at 50 to 500 person hospitality groups in the UK and Ireland with no applicant tracking system" is the difference between a campaign that limps and one that books meetings. Vague ICPs produce vague lists, and vague lists produce vague results.

## When to use

- At the very start, before any list building or copywriting
- When the user asks any downstream question ("write my copy", "build my list") and `profiles/profile.yaml` does not exist
- When the ICP has genuinely changed (new market, new product) and the profile needs a rebuild

## The output: profile.yaml schema

Save to `profiles/profile.yaml`. This is the contract; downstream skills depend on these exact top-level keys:

```yaml
company:
  name: Fernwood Analytics
  website: fernwoodanalytics.example
  one_liner: Revenue forecasting software for mid-market ecommerce brands
  tone: casual            # casual | formal | peer-to-peer

offer:
  service: what they actually sell, one sentence
  pricing_model: monthly  # one-time | monthly | annual | usage-based | per-project
  entry_point: cheapest, lowest-commitment way someone can start (tier, pilot, trial)
  risk_reversal: any guarantee, free trial, or pay-on-results deal they can stand behind
  current_cta: what they currently ask prospects to do

icp:
  industries:
    include:
      - Ecommerce
      - Consumer Goods
    exclude:
      - Agencies
  titles:
    - Head of Finance
    - VP Finance
    - Finance Director
  size:
    employees_min: 50
    employees_max: 500
  geography:
    countries: [GB, IE]
    regions: []           # optional, e.g. US states
  excluded_domains:
    - competitor.example
    - existing-customer.example

proof_points:
  - "Cut forecast error from 22% to 6% for a 300-person homeware brand"
  - "Used by finance teams at 40+ mid-market ecommerce companies"

case_studies:
  - customer: Hartley & Vane (fictional example)
    industry: Ecommerce
    result: "Went from 3 days of manual forecasting per month to 2 hours"
    cleared_for_cold_email: true   # can we NAME them in cold copy?

calendar_link: https://cal.example/founder/intro
created: 2026-07-31
```

Every field matters downstream. `offer.pricing_model`, `offer.entry_point`, and `offer.risk_reversal` decide the entire offer strategy in `/offer-framing`. `cleared_for_cold_email` decides whether copy says "Hartley & Vane" or "a 300-person homeware brand". `calendar_link` is what reply handling anchors bookings to.

## Steps

### 1. Ask for the website, then scrape it

Before any interview question, ask:

> "What's the website of the business you're running cold email for?"

Fetch the homepage plus the obvious linked pages (about, pricing, case studies, customers). Then play back a summary BEFORE asking anything else:

> "Here's what I understand from your site: **{Company}** sells {product in one sentence} to {who, based on case studies and homepage}. The core value prop looks like {summary}. Social proof I found: {logos, metrics, testimonials}. Pricing signal: {visible pricing / hidden / trial available}.
>
> Proposed starting ICP: {titles + industries + size inferred from the case studies}.
>
> Anything wrong before I ask the detailed questions?"

The summary is the anchor, not the answer. Their corrections here save ten minutes of interview. Case studies and logo walls are the best ICP signal on any website: who a company brags about serving is who they actually serve.

No website? Ask for a two-sentence description of what they sell and to whom, and use that as the anchor instead.

### 2. Interview, one question at a time

Do not batch these. Ask, wait for the answer, ask the next. Pre-fill proposed answers from the scrape wherever possible ("your site suggests X, confirm or correct?").

1. **What do you sell, in one sentence?** No jargon. If the answer needs three sentences, it needs sharpening.
2. **Name your single best customer.** Not a category, a name. If they hesitate, ask for the last three customers by revenue or by fit. This one answer anchors everything: the industries, the size band, the pain, the proof.
3. **What job title actually buys this?** Push back on "the CEO" for anything above 50 employees. It is usually not true, and titles drive the whole list build.
4. **Company size: hard minimum and hard maximum, in employees.** Numbers, not words. "SMB" and "enterprise" are not answers.
5. **Industries: which are in, which are explicitly out?**
6. **Geography: which countries?** Regions or states only if they genuinely matter.
7. **Anyone you must NOT email?** Competitors, existing customers, partners. Collect domains for the exclusion list.
8. **Pricing model, and the cheapest way someone can start.** Monthly? Annual contract only? Is there a pilot, a trial, a small first project? This question decides the offer strategy later, so dig properly.
9. **What could you guarantee, or give away free, and stand behind it?** A guarantee, a free trial, pay-on-results, a free sample of the work. Most people say "nothing" at first. Push once: "what are you confident enough in that you would bet on it?"
10. **Proof points: your two or three best, with real numbers.** "We're great at X" is not a proof point. "Cut forecast error from 22% to 6%" is.
11. **Which customer names are cleared to appear in a cold email?** Get explicit yes/no per name. Anything not cleared gets described anonymously in copy ("a 300-person homeware brand").
12. **Your booking link.** The actual URL. Reply handling needs it; cold emails never contain it.
13. **Tone: casual or formal? Peer or vendor?** Default to casual peer-to-peer unless their market punishes it (legal, finance, compliance).

### 3. Pressure-test before writing

Three checks before the file gets written:

- **The named-customer check.** Does the best customer from question 2 actually fit the ICP from questions 3 to 6? If their favourite customer is a 2,000-person company and their stated max is 500, one of those answers is wrong. Ask which.
- **The breadth check.** Multiply it out roughly: industries x size band x geography. If the reachable market is a few hundred companies, say so now, because cold email needs volume and a tiny TAM changes the whole plan. If it is enormous ("all B2B, 10 to 10,000 employees, global"), it is not an ICP yet; tighten to where the best customers cluster.
- **The first-listed-country trap.** If they listed four countries, do not assume the first one is the main market. Ask.

### 4. Write the file and confirm

Write `profiles/profile.yaml` using the schema above. Print the full yaml back and ask "does this look right?" Fix anything they flag, then confirm it is saved. Tell them plainly: this file drives everything, and they can rerun `/icp-builder` any time it needs updating.

## Example: a completed intake

Fictional company, so you can see the shape of good answers:

- **Company:** Kestrel Compliance, sells SOC 2 readiness software to startups
- **Best customer:** "Loomfield, a 120-person fintech, closed in 9 days"
- **Title:** Head of Engineering (they tried CTOs, too senior; ops managers, too junior)
- **Size:** 30 to 300 employees, hard on both ends
- **Industries:** fintech and healthtech in, agencies and ecommerce out
- **Geography:** US and UK
- **Pricing:** monthly, and there is a free readiness scan as an entry point
- **Risk reversal:** "if the scan finds nothing worth fixing, we say so and leave you alone"
- **Proof:** "took Loomfield from zero to audit-ready in 6 weeks", cleared to name
- **Tone:** casual, engineer-to-engineer

That free readiness scan and the willingness to name Loomfield are the two most valuable lines in the whole profile. `/offer-framing` will build the entire campaign around them.

## Common mistakes

- **Accepting category words as answers.** "SMBs", "enterprise", "decision-makers". Push for numbers and titles every time.
- **Letting them describe their aspiration instead of their reality.** The ICP is who buys today, evidenced by named customers. Who they want to sell to next year is a separate conversation.
- **Skipping the pricing and risk-reversal questions because they feel like offer territory.** They ARE offer territory, and this is exactly where to capture them. `/offer-framing` cannot run properly without them.
- **Writing titles without synonyms.** "VP of Talent" alone will miss "Head of People", "Chief People Officer", "VP People Operations". Capture the family, not one string.
- **Treating nice-to-haves as filters.** "Recently funded" or "using a specific tool" are prioritisation signals, not hard filters. Make them hard filters and the list shrinks 20x.
- **Guessing instead of asking.** If the scrape is thin or ambiguous, ask. A wrong profile.yaml silently poisons every downstream skill.

## What to do next

With `profiles/profile.yaml` written, go straight to `/offer-framing`. The offer, not the copy, is what decides whether this campaign works, and the profile now contains everything that skill needs (pricing model, entry point, risk reversal, proof).

## Related skills

- `/cold-email-kickoff`: the guided entry point that runs this skill as its first step
- `/offer-framing`: turns the offer section of this profile into an offer that converts cold traffic
- `/ai-ark-list-builder`: turns the icp section into an actual lead list
- `/cold-email-copywriting`: reads tone, proof points, and case studies from this profile
