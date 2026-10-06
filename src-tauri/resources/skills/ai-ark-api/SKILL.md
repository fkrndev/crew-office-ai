---
name: ai-ark-api
description: Pure API reference for AI Ark, the lead database behind /ai-ark-list-builder. Endpoint table, X-TOKEN auth, the three filter shapes (SearchMatch, AllAny, Range), the company-keyword "sources" shape that returns 401 when you get it wrong, export polling, rate limits, and every trap that produces silent wrong results. Consult whenever writing code against the AI Ark API, debugging a search that returns unfiltered or zero results, hitting 401/400/409/429 responses, or extracting emails from an export payload. This is a reference skill, not a workflow.
user_invocable: false
---

# AI Ark API Reference

The raw mechanics behind `/ai-ark-list-builder`. Everything here was verified the expensive way. If a search behaves strangely, the answer is almost certainly on this page.

## Basics

- **Base URL:** `https://api.ai-ark.com/api/developer-portal/v1`
- **Auth:** `X-TOKEN: <your key>` header. NOT a Bearer token.
- **`Content-Type: application/json` on EVERY call, including GETs.** Omitting it on a GET causes failures that look like auth problems.
- **Env var:** `AI_ARK_API_KEY` (see the repo `.env.example`)

## Endpoints

| Method | Path | What it does |
|---|---|---|
| POST | `/companies` | Company search |
| POST | `/people` | People search (preview; size 0-100) |
| POST | `/people/export` | Async paid export, max 10,000 per request, ~1 credit per record |
| GET | `/people/export/{trackId}/statistics` | Poll export state |
| GET | `/people/export/{trackId}/inquiries?page=&size=` | Page export results (size up to 100) |
| GET | `/payments/credits` | Credit balance |

Search body shape: `{ account?, contact?, page, size }`. `page` is 0-based. `size` is 0-100 for search, 1-10,000 for export.

## The three filter shapes

Getting the shape wrong does not error politely. It either silently ignores your filter or throws a misleading status code. There are exactly three shapes:

### 1. SearchMatch (industries, technologies, keyword, titles)

```json
{
  "any": {
    "include": { "mode": "WORD", "content": ["logistics", "freight"] },
    "exclude": { "mode": "WORD", "content": ["staffing", "recruiting"] }
  }
}
```

`mode` is one of `SMART` (fuzzy, broadest), `WORD` (word-boundary), `STRICT` (near exact). Same word list, very different counts, so mode is a precision dial independent of the words.

**Trap:** the documented-looking flat form `{"any": {"mode": "...", "content": [...]}}` does not error. It **silently returns UNFILTERED results.** If a filtered search returns roughly the same count as no filter, you used the flat shape.

### 2. AllAny (location, seniority, domain)

```json
{
  "any": {
    "include": ["Europe::Germany", "Europe::United Kingdom"],
    "exclude": []
  }
}
```

Plain string arrays, no mode.

### 3. Range (employee count, tenure, funding)

```json
{ "type": "RANGE", "range": [{ "start": 50, "end": 500 }] }
```

Always give an explicit upper bound. There is no open-ended range; for "1,000+" use something like `{"start": 1001, "end": 1000000}`.

## The company-keyword "sources" shape (the 401 trap)

Company keyword filters on `account.keyword` need a fourth, richer shape on the exclude side. Each entry in `sources` says which company field to match against:

```json
{
  "account": {
    "keyword": {
      "any": {
        "exclude": {
          "sources": [
            { "mode": "WORD", "source": "DESCRIPTION" },
            { "mode": "WORD", "source": "SEO" },
            { "mode": "WORD", "source": "KEYWORD" },
            { "mode": "WORD", "source": "NAME" }
          ],
          "content": ["agency", "consultancy", "staffing", "wholesale"]
        }
      }
    }
  }
}
```

- `source` enum (company level): `NAME | KEYWORD | SEO | DESCRIPTION | INDUSTRY`
- **Wrong shape** (a plain `{mode, content}` object without `sources`) returns **401**. It looks like an auth failure. Your key is fine, your JSON is wrong.
- **Wrong `source` value** (for example a contact-level enum value) returns **400 "request not readable"**.
- Matching `DESCRIPTION + SEO` reproduces what the web UI calls company keyword exclusion; adding `KEYWORD + NAME` also strips companies caught by their name or tags (for example "Northwind Consultancy").

## Person keywords: don't

Person-level keyword filters (`contact` keyword fields) are unreliable through the API. Put ALL keyword logic on `account.keyword`. Title filtering on the contact (`contact.experience.current.title` include/exclude) works fine, so titles plus company keywords covers almost every real targeting need.

## Export flow

1. `POST /people/export` with your search body **plus a `webhook` field. The webhook is REQUIRED**, the call 400s without it. Pass a placeholder URL you control; you'll poll instead of waiting for the callback.
2. Response: `{ "trackId": "...", "state": "PENDING" }`
3. Poll `GET /people/export/{trackId}/statistics` every 10-30 seconds. **A 409 means still processing, not an error.** Retry until state is `DONE` (or `COMPLETED`; treat `FAILED` as terminal).
4. Page `GET /people/export/{trackId}/inquiries?page=N&size=100` until the envelope says `last: true`.

Result envelope is Spring-style:

```json
{ "content": [ ... ], "last": false, "totalElements": 8412 }
```

**The email lives at `content[i].email.output[0].address`.** Not at a top-level `email` field. Everyone trips on this once.

## Rate limits

5 requests/second, 300/minute, 18,000/hour. On a 429, back off exponentially. For export paging at size 100 you will rarely hit these unless you run jobs in parallel.

## Gotchas checklist

- **No free count endpoint.** `POST /people` requires size >= 1 and any size >= 1 bills for a profile. On a zero-credit account it 403s. The web UI shows counts for free.
- **Results are NOT randomly ordered.** Worst matches cluster at the back. Quality-check the last pages, not page 1.
- **`job_duration` values are in MONTHS.** `job_duration_min: 6` means six months of tenure, not six years.
- **Invalid department values return 0 results silently**, no error. An empty department-based search means "check the enum string" before it means "no such people exist".
- **Seniority enum** (verified): `founder owner partner c_suite vp director head manager senior mid-level entry intern`. A typo is silently ignored.
- **Location format** is `Continent::Country`, for example `Europe::Germany`, `North America::United States`.
- **Search filtered count sanity check:** before an export, run the same body through `POST /people` with size 1 and confirm the total dropped versus the unfiltered version. If an exclude didn't move the count, the shape is wrong (see the 401 trap above).

## What to do next

You don't invoke this skill directly; it backs `/ai-ark-list-builder`. If you landed here debugging, fix the filter shape, re-run the size-1 sanity check, and go back to the builder flow. Never re-submit an export until the preview count confirms the filter actually applied.

## Related skills

- `/ai-ark-list-builder` - the workflow this reference supports
- `/list-hygiene` - what happens to the export before it can be sent to
- `/email-verification` - the rule on (not) verifying AI Ark exports
