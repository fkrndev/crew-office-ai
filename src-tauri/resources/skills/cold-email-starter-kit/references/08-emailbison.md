# 08: EmailBison

EmailBison is the sending platform in this stack: it holds your sender inboxes, your campaigns, your sequences, and your leads, and it does the actual sending. Its API is workable but it lies in specific, repeatable ways. Every claim in this doc was verified against the live API the hard way, so read the traps table even if you read nothing else.

This is the orientation. The full references are `/emailbison-api` (every endpoint and trap) and `/emailbison-campaign-launch` (the build flow).

---

## Setup

EmailBison is white-labelled, so there is no universal API URL. Your instance has its own domain, and that's your base URL.

```bash
# .env
EMAILBISON_BASE_URL=https://send.yourinstance.com   # no trailing slash
EMAILBISON_API_KEY=your_token                       # workspace Settings > API > create token
```

Two things to internalize on day one:

1. **Keys are hard-scoped to one workspace.** A key can never see another workspace's campaigns. "Zero results" with the wrong key looks exactly like an empty workspace.
2. **Workspace creation is UI-only.** Every key type gets a 403 with a misleading message. Create the workspace by hand in the UI; from there the API takes over.

Auth is `Authorization: Bearer <key>` plus `Accept: application/json`, and `Content-Type: application/json` on writes. Run `npm run verify` after setting the two env vars; it does a real call and tells you if the key works.

---

## The pagination law

**15 results per page, always.** `per_page` and `limit` are silently ignored. Loop `?page=N` until `meta.last_page`; `meta.total` on page 1 gives you the true count without walking the pages.

The corollary you'll hit personally: **a result count of exactly 15 is presumptively a bug in your code.** It means you read page 1 and stopped. This exact mistake has reported "15 senders" on workspaces holding hundreds. If any number comes back as exactly 15, assume you forgot to paginate until proven otherwise.

Big sets: there's a hard cap at 1000 pages; past that, switch to `?pagination_type=cursor` and follow `meta.next_cursor`. One exception: the sequence-steps read is unpaginated (no `meta` in the response means take page 1 and stop). Full mechanics in `/emailbison-api`.

---

## The traps, in one table

| Trap | What happens | The defence |
|---|---|---|
| **Auth failures look like 200s** | A bad key does NOT 401. It returns 200-shaped JSON with `data` as an OBJECT (`{"data":{"success":false,...}}`) where you expected an array | Check `Array.isArray(data)` before iterating. `data`-is-an-object means auth error, never "zero results" |
| **PATCH resets booleans** | `PATCH /api/campaigns/{id}/update` resets every boolean you don't include. A limits-only patch silently flips `plain_text` and friends on a live campaign | Re-send the FULL boolean set on every settings change, then GET and diff. "Settings drift" is always this |
| **Resume is the only activation** | `/activate` and `/start` don't exist; writing `status: "active"` is silently ignored | `PATCH /api/campaigns/{id}/resume` activates everything: draft to active, paused to active. It needs a schedule, a step, leads, AND senders, or you get an opaque "This campaign is incomplete" |
| **Sequence PUT orphans variants** | Editing a variant via PUT returns 200 but silently clears its parent link, turning it into a phantom sequential step (every lead then gets it as an extra email) | Never PUT-edit a variant. DELETE and re-POST instead. Also: reads return `variant_from_step` but writes need `variant_from_step_id`, and the PUT URL takes the campaign's `sequence_id`, not a step id |
| **`custom_variables` shape** | A flat map (`{"CITY": "London"}`) gets a 422 | It must be an array of objects: `[{"name": "CITY", "value": "London"}]`. Names UPPERCASE on write (the API lowercases them on read), and never name one `COMPANY`, it collides with the native tag |

Two smaller ones worth knowing before they bite: bounced leads refuse attach with a 422 (filter them out, don't fight the platform's memory), and the schedule's `status` label is unreliable in both directions (`emails_sent > 0` is the only real proof of life).

The meta-rule for every write: **act, re-read, diff.** GET the object after every PATCH or POST and compare. The API confirms nothing you don't check yourself.

---

## The three scripts

All in `/emailbison-campaign-launch`'s `scripts/` folder.

**`emailbison-create-campaign.ts`** creates the campaign as a draft, loads every copy variant of each step in one sequence-steps POST (first item primary, the rest `"variant": true`), applies the deliverability settings (`plain_text` by campaign type, `open_tracking: false`, auto-replies out of stats, daily limits at 1000+ because senders govern volume, not campaign caps), attaches ALL workspace senders, and sets the schedule (weekdays, business hours, the RECIPIENTS' timezone).

**`emailbison-upload-leads.ts`** takes your hygiene-passed CSV and does the two-phase load: bulk upsert (`/api/leads/create-or-update/multiple`, batches of ~140), then attach to the campaign (batches of 500, idempotent). Custom variables go on at upsert in the array shape above. It reconciles `total_leads` against your source list at the end; if the count is short, find out why before moving on.

**`emailbison-pull-analytics.ts`** reads the numbers back: campaign stats (a POST, not a GET), daily volume via the chart endpoint (the only true daily read; cumulative `emails_sent` can't tell you what sent today), and the bounced replies feed, which is where actual bounce reasons live.

---

## The draft-only policy

**Nothing in this repo activates a campaign. Ever.**

The scripts build to `status: "draft"` and stop. There is no activate flag, and I won't add one: the moment a script can press start, someone's unreviewed campaign goes live at 9am with a broken variable in the subject line. The launch sequence is always:

1. Build with the scripts above (campaign lands as draft)
2. Run `/pre-launch-qa`, every item, PASS/FAIL
3. **You press start yourself**, in the UI or by typing the resume call with your own hands

The QA gate only works if nothing can go around it.

---

## Copy variables, quick reference

- `{{DOUBLE_CURLY_UPPERCASE}}` throughout
- Native tags: `{{FIRST_NAME}}`, `{{COMPANY}}`, `{{SENDER_FIRST_NAME}}`
- `<br>` for line breaks in HTML bodies (only relevant when `plain_text: false`, which in this system means reply follow-up campaigns only, see `09-reply-handling.md`)

---

## What to do next

If you're building a campaign right now, go to `/emailbison-campaign-launch` and let it drive. If you're debugging a weird API response, `/emailbison-api` has the full endpoint-by-endpoint reference with every trap documented. And before anything goes live: `11-pre-launch-checklist.md`, then `/pre-launch-qa`.
