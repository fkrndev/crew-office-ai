---
name: ai-ark-list-builder
description: Build a cold email lead list end to end with AI Ark, from targeting definition to a clean leads.csv. Triggers on "build a list", "build me a lead list", "find prospects", "AI Ark export", "more leads", "expand the list", "the list is too small", "the list is full of junk", "targeting for my campaign", "export leads to CSV", "how many leads can I get". Covers titles include/exclude, industries, employee size, geography, and keyword logic, plus the preview-then-export flow that stops you burning credits on a bad filter. Exports cost about 1 credit per record, so this skill always previews first and confirms before spending.
---

# AI Ark List Builder

I build every cold email list the same way: define the targeting precisely, preview the search for free, check the quality on the WORST pages of the results, tighten, and only then export. This skill runs that loop.

List building is a craft, not a checklist. A list is never perfect. The goal is the right volume of the right people, then tightening from there.

## The money rule (read this before anything else)

- **Previews are free. Exports cost about 1 credit per record.** A sloppy 20,000-record export of the wrong people is real money gone.
- **There is no free count endpoint.** You read a search's total either from a size-1 search call (which reveals one billable profile) or from the AI Ark web UI, where the count is on screen for free.
- **Never export without confirming first.** The script below prints the count and estimated credit cost and waits for an explicit yes. Claude should never skip that confirmation on your behalf, and neither should you.

## When to use

- Building a fresh list for a new campaign or a new client of yours
- Expanding a list that's running dry (a working campaign wants roughly 50k sends a month of headroom)
- Fixing a noisy list that's dragging in the wrong people
- Any time you're about to spend AI Ark credits

If you already have a list from somewhere else (scraped, bought, CRM export), skip this and go straight to `/email-verification` and `/list-hygiene`.

## Step-by-step process

### 1. Define the targeting

Pull the ICP from `profiles/profile.yaml` (built by `/icp-builder`). Turn it into concrete filter values, never prose. "Expansion-stage companies" is not a filter; "50-500 employees" is. You need:

- **Titles to include** (exact titles, or seniority + department)
- **Titles to exclude** (always, see below)
- **Industries** (include a few clean ones, or exclude the bad ones and keep the rest)
- **Employee size range** (e.g. 50-500)
- **Geography** (country level)
- **Keyword logic** where the industry labels can't express the ICP

Fictional example, a company selling routing software to logistics firms:

```
titles_include:  Head of Operations, VP Operations, Operations Director, COO
titles_exclude:  Assistant, Coordinator, Intern, Analyst, Driver, Warehouse
industries:      logistics and supply chain, freight and package transportation
size:            50-500
geography:       United Kingdom, Germany, Netherlands
exclude_keywords: agency, consultancy, recruiting, software, staffing
```

**Title strategy, in order of preference:**

1. **Strict exact titles.** Cleanest, lowest volume. Use when the ICP is a small well-defined set and volume isn't the constraint.
2. **Seniority + department, then exclude hard.** My usual default. Set seniority (c_suite, vp, director, head), set the function, then build a generous exclude list, because irrelevant titles ALWAYS leak in. The exclude list is the single highest-leverage step for list quality, and it is never "done".
3. **Seniority baked into the title box** ("Director of Operations", "Head of Marketing") when option 2 is still too broad.

When you're not sure which way to define the person, build 2-3 preview routes side by side and compare count AND back-page quality. The widest construction that's still all buyers wins, not the biggest number.

**Beware generic single words.** They balloon volume and dilute. "Operations" catches every VP of Operations in every industry. "Security" catches security guards. "Editor" catches copy editors when you wanted video editors. When one word doubles the count, check the back pages, it's usually diluting, not widening. Anchor ambiguous titles with the discipline word and exclude the adjacent meanings.

**Keyword logic goes on the COMPANY, not the person.** Person-level keyword filters are unreliable in the AI Ark API. Put keyword includes and excludes on `account.keyword` (company name, description, SEO text). Full mechanics in `/ai-ark-api`.

### 2. Preview the search (free)

Run the search without exporting. Read the total count per segment and compare it to your volume target. If a segment comes back short, widen in this order (least to most quality risk): loosen the size band, add adjacent geographies, loosen industry (or flip from include-list to exclude-list), broaden titles, add a keyword angle.

### 3. Check the BACK pages, not the front

AI Ark results are not randomly ordered. **The worst matches cluster at the end.** Page 1 always looks great, so judging a list by page 1 is how people export junk.

`--preview` already fetches both ends: the front page AND the true back page, in one call. Read the back page it prints. If it's full of off-ICP titles, wrong-size companies, or wrong geographies, the filter is too loose: add excludes and tighten. If the back page is still clean, the count is real and you can ship. Do this before every export, no exceptions.

One caveat: past roughly 10,000 matches, the API caps how deep it'll page, so the script shows you the deepest page it can reach instead of the true tail, and says so. If you're seeing that message, the count alone is telling you the same thing: a search that wide is diluting, not widening. Narrow it.

### 4. Tighten or widen, re-preview, loop

Iterate until every segment clears its volume share AND the back page is clean. Report the count every pass. Never say "looks fine" without the number.

### 5. Export (confirm first)

The export is asynchronous: the API takes the search, returns a tracking ID, processes in the background, then you page through the results. The reference script handles all of it, driven by a targeting config file:

```json
// targeting.json
{
  "titlesInclude": ["Head of Operations", "VP Operations", "Operations Director"],
  "titlesExclude": ["Assistant", "Coordinator", "Intern", "Analyst"],
  "industriesInclude": ["logistics and supply chain"],
  "employeeSize": [50, 500],
  "locations": ["Europe::United Kingdom", "Europe::Germany", "Europe::Netherlands"],
  "companyKeywordExclude": ["agency", "consultancy", "recruiting"],
  "size": 2000
}
```

```bash
npx tsx ${CLAUDE_SKILL_DIR}/scripts/ai-ark-export.ts --preview --config targeting.json   # free: total, front page, AND back page
npx tsx ${CLAUDE_SKILL_DIR}/scripts/ai-ark-export.ts --submit --config targeting.json    # confirms cost, then starts the export
npx tsx ${CLAUDE_SKILL_DIR}/scripts/ai-ark-export.ts --fetch                             # polls to DONE, writes export-leads.csv
```

What the submit/fetch pair does:

1. Shows the export size, cost (about 1 credit per record), and your credit balance, then **stops and asks you to confirm** before spending anything
2. Submits the export with a placeholder webhook URL (the API requires a webhook field even if you poll)
3. Persists the tracking ID to `.aiark-tracks.json`, so a killed run resumes with `--fetch`
4. Polls until the export finishes (a 409 while polling just means still processing, it keeps waiting)
5. Pages through the results and writes `export-leads.csv`

The `size` field in the config controls spend. Max 10,000 records per export request; split bigger jobs into multiple configs (for example by employee-size band).

### 6. Hygiene before anything touches a sending tool

The export is not send-ready. Run `/list-hygiene` next: dedup, exclusion lists, and the mandatory MX check that silently drops Mimecast and Barracuda protected domains. That step is automatic and non-negotiable, it applies to AI Ark lists exactly like every other source.

One thing you do NOT do: re-verify the emails. AI Ark exports are verified at source. See `/email-verification` for why paying to verify them again is a beginner mistake.

## Common gotchas

- **No free count API.** Any API search with size 1 or more bills for at least one profile. The web UI shows counts for free.
- **Person-level keywords are unreliable.** Put keyword logic on the company (`account.keyword`), not the contact. Details in `/ai-ark-api`.
- **The webhook field is required on export** even when you intend to poll. Pass a placeholder URL you control; the script does this for you.
- **409 during polling means still processing.** It is not an error. Keep polling every 10-30 seconds.
- **Results are ordered best-first**, which is exactly why the back-page check exists. The last pages are the honest ones.
- **Job duration filters are in MONTHS, not years.** A tenure filter of 6 means 6 months.
- **A typo'd seniority or department value fails silently.** You get zero results, not an error. If a route comes back empty, suspect the enum string first.
- **Every industry tag drags in a crowd you don't want.** A tag that's half off-ICP is worse than no tag. Audit each one against the back pages, and pair broad tags with company-description keyword excludes.
- **Don't gate on skills data.** LinkedIn skills fields are sparse and stale, a skills filter silently deletes real buyers. Anchor titles and exclude instead.

## What to do next

Run `/list-hygiene` on the export (dedup + exclusions + the MX check). Then `/cold-email-copywriting` to write to this exact audience, and `/emailbison-campaign-launch` to upload as a draft.

If the list came back small even after widening, the ICP may be too narrow. Revisit `/icp-builder` before forcing volume with junk filters.

## Related skills

- `/ai-ark-api` - the raw API reference behind this skill (filter shapes, endpoints, traps)
- `/list-hygiene` - dedup, exclusion lists, and the mandatory MX exclusion step
- `/email-verification` - when to verify (your own lists) and when not to (AI Ark exports)
- `/icp-builder` - produces the profile.yaml this skill reads
- `/cold-email-copywriting` - write the emails for the list you just built
- `/emailbison-campaign-launch` - upload the finished list as a draft campaign
