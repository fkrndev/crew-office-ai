# 06: List Building (condensed)

Every list in this system comes from AI Ark, and every list follows the same loop: define targeting precisely, preview for free, judge quality on the WORST pages, tighten, and only then pay for the export. Then hygiene before anything touches a sending tool. Full skills: `/ai-ark-list-builder`, `/email-verification`, `/list-hygiene`.

**The money rule first: previews are free, exports cost about 1 credit per record, and there is no free count endpoint in the API** (the web UI shows counts free). A sloppy 20,000-record export of the wrong people is real money gone, which is why the flow below never exports without a confirmation.

## 1. Define the targeting

Turn your ICP (from `/icp-builder`'s `profiles/profile.yaml`) into concrete filter values, never prose. "Expansion-stage companies" is not a filter; "50-500 employees" is. You need: titles to include, titles to EXCLUDE (always), industries, employee size range, geography, and keyword logic where industry labels can't express the ICP.

Fictional example, selling routing software to logistics firms, as a `targeting.json`:

```json
{
  "titles": ["Head of Operations", "VP Operations", "Operations Director", "COO"],
  "exclude_titles": ["Assistant", "Coordinator", "Intern", "Analyst", "Driver"],
  "industries": ["logistics and supply chain", "freight and package transportation"],
  "size": [50, 500],
  "locations": ["United Kingdom", "Germany", "Netherlands"],
  "exclude_keywords": ["agency", "consultancy", "recruiting", "staffing"]
}
```

(The exact accepted fields are documented in the header of the export script; all optional except size for a submit.)

Three targeting rules that do most of the work:

- **The exclude list is the highest-leverage step.** Irrelevant titles ALWAYS leak in. Build a generous exclude list and keep growing it; it is never "done".
- **Beware generic single words.** "Operations" catches everyone; "Security" catches guards. When one word doubles the count, it's usually diluting, not widening.
- **Keyword logic goes on the COMPANY, not the person.** Person-level keyword filters are unreliable in the AI Ark API; put includes and excludes on the company record. Mechanics in `/ai-ark-api`.

## 2. Preview (free)

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts --preview --config targeting.json
```

Read the total count against your volume target. Too small? Widen in order of least quality risk: loosen the size band, add adjacent geographies, loosen industry, broaden titles. When unsure how to define the person, run 2-3 preview routes side by side; the widest construction that's still all buyers wins, not the biggest number.

## 3. Quality-check the BACK pages

AI Ark results are not randomly ordered: **the worst matches cluster at the end.** Page 1 always looks great, which is exactly how people export junk. `--preview` fetches the front page AND the true back page automatically. Off-ICP titles, wrong sizes, wrong geographies back there? The filter is too loose: add excludes and tighten. Back page still clean? The count is real. Do this before every export, no exceptions. (Past ~10,000 matches the API caps how deep it can page, so you'll see the deepest reachable page instead, which is itself a sign to narrow.)

## 4. Export (confirm first), then fetch

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts --submit --config targeting.json
```

Prints the count and estimated credit cost, then **stops and asks for your explicit yes** before spending. Exports are asynchronous: the script submits (with the placeholder webhook URL the API requires even when polling), records the tracking ID to disk so a killed run is resumable, and then:

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/ai-ark-list-builder/scripts/ai-ark-export.ts --fetch
```

polls until done (a 409 while polling just means still processing) and pages the results out to CSV. Max 10,000 records per export request; bigger jobs get split.

## 5. The verification rule

- **AI Ark exports: NEVER re-verify.** They are verified at source. Running them through a verifier adds cost and time and changes nothing. Anyone telling you to "always verify everything" is selling verification credits.
- **Lists from anywhere else: ALWAYS verify with Kitt.** Scraped, purchased, CRM exports, old campaign lists, partner CSVs. Outside lists routinely run 10-30% invalid, and sending to that burns your domains for weeks.

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/email-verification/scripts/kitt-verify.ts --csv leads.csv --cap 500
```

Resumable, capped per run, confirms cost before spending, and holds concurrency at 3 because the realtime API throttles hard above that. Keep `valid`, treat `valid-risky` as catch-all (a mail server that accepts mail to ANY address at the domain, so the verifier can't confirm the specific mailbox is real, a judgment call by segment), drop `invalid`. Full detail in `/email-verification`.

## 6. Hygiene: always, every list, every source

No export is send-ready. Three steps, in order, and AI Ark lists get all of them too:

1. **Dedup**, by email AND by company domain (one contact per company for most cold outbound; keep the dropped contacts as a replacement pool).
2. **Exclusion lists:** your customers, competitors, past bounces, unsubscribes, open conversations. By domain, append-only, forever.
3. **The MX check, mandatory and automatic:**

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/list-hygiene/scripts/mx-exclusion-check.ts --csv leads.csv
```

Mimecast and Barracuda protected domains reject cold mail from new senders on POLICY, at the connection level, regardless of address validity. They're a small share of any list and an outsized share of all bounces. Verification cannot fix this because it isn't an address problem, which is why AI Ark lists skip verification but never skip the MX check.

Full doctrine in `/list-hygiene`.

## Common traps

- **Judging by page 1.** The back pages are the honest ones.
- **A typo'd enum (seniority, department) fails silently** with zero results, not an error. Empty route? Suspect the string first.
- **Job duration filters are in MONTHS**, not years.
- **Every industry tag drags in a crowd.** A tag that's half off-ICP is worse than no tag; pair broad tags with company-keyword excludes.
- **Forcing volume with junk filters.** If the list is small after honest widening, the ICP is too narrow; fix it in `/icp-builder`, not with looser excludes.

## Next

Copy for this exact audience (`03-copywriting.md`), then the campaign build with `/emailbison-campaign-launch`: one campaign per list, all variants in one step, DRAFT only, `/pre-launch-qa` before you press start.
