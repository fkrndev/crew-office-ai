---
name: emailbison-api
description: "The EmailBison API reference: Bearer auth, per-instance base URLs, the pagination law (15 per page always, loop to meta.last_page, cursor pagination for big sets), the PATCH-resets-booleans trap, resume as the only activation, sequence-step landmines (variant_from_step vs variant_from_step_id, PUT orphans variants, Re: prefixing), lead upsert and attach batch sizes, custom_variables shape, stats endpoints, schedule shape. Load whenever any other skill or script talks to EmailBison: campaign create, settings update, sequence steps, lead upload, sender attach, schedule, stats, bounces. Every claim here was verified against the live API the hard way."
user_invocable: false
---

# EmailBison API Reference

Everything in this file was learned by getting it wrong against the live API first. EmailBison's API is workable but it lies in specific, repeatable ways: auth failures look like success, pagination silently truncates, and one PATCH can flip settings on a live campaign. Read the three landmines even if you read nothing else.

## Auth and base URL

- Base URL is **per instance** (EmailBison is white-labelled, there is no universal default). Set `EMAILBISON_BASE_URL` in `.env`.
- Auth: `Authorization: Bearer <key>`, plus `Accept: application/json` and `Content-Type: application/json` on writes. Set `EMAILBISON_API_KEY`.
- Keys are hard-scoped to one workspace. A key can never see another workspace's campaigns; "zero results" with a wrong key looks like an empty workspace.
- Workspace creation is UI-only. Every key type gets a 403 with a misleading message. Create the workspace in the UI, then the API takes over.

### Landmine 1: auth failures look like success

A bad or mismatched key does NOT return 401. It returns 200-shaped JSON:

```json
{"data": {"success": false, "message": "..."}}
```

On any list endpoint, if `data` is an **object** where you expected an **array**, treat it as an auth or shape error. Never interpret it as "zero results". Every script should check `Array.isArray(data)` before iterating.

## Landmine 2: THE PAGINATION LAW

- **15 per page, always.** `per_page` and `limit` are silently ignored.
- Loop `?page=N` until `meta.last_page`. `meta.total` on page 1 gives you the true count without walking the pages.
- **A result count of exactly 15 is presumptively a page-1-only bug in your code.** This exact mistake has reported "15 senders" on workspaces that had 100, 800, and 1000. If a number comes back as exactly 15, assume you forgot to paginate until proven otherwise.
- **Hard cap at 1000 pages** (15,000 rows). Page 1001+ returns `{"message":"You are requesting too many pages."}` as a plain object, which crashes naive loops.
- **The cap is not a wall: use cursor pagination.** `?pagination_type=cursor`, then follow `meta.next_cursor` via `?cursor=...` until it's null. Sequential (no parallel fetch) so slower, but it reaches every row. For any full-set audit, assert `fetched == meta.total` at the end; a short read voids the audit.
- **Exception:** `GET /api/campaigns/{id}/sequence-steps` is NOT paginated. No `meta` in the response; `?page=2` returns identical rows forever. Rule: no `meta.last_page` in the response means take page 1 and stop. On a campaign with no steps yet, it returns the Landmine-1 error shape; that's an empty set, not a failure.

## Landmine 3: PATCH update resets unspecified booleans

`PATCH /api/campaigns/{id}/update` **resets every boolean you don't include**. A limits-only patch will silently flip `plain_text` and `include_auto_replies_in_stats` on a live campaign. This is also why settings appear to "drift": they don't, a partial PATCH reset them.

Every campaign PATCH must re-send the full boolean set:

```json
{
  "plain_text": true,
  "open_tracking": false,
  "can_unsubscribe": true,
  "reputation_building": false,
  "include_auto_replies_in_stats": false
}
```

`plain_text` depends on the campaign `type`: read it off the campaign first, never hardcode. `outbound` with plain-text copy: `true`. `reply_followup`: always `false` (the follow-up bodies carry an HTML calendar link; plain text flattens it into a dead line).

**The meta-rule for every write: verify, act, re-read, diff.** GET the object after every PATCH and compare before/after.

## Campaigns and activation

- Create: `POST /api/campaigns {"name": "...", "type": "outbound"}` (or `"reply_followup"`). Arrives as `status: "draft"`.
- **There is no launch endpoint and no status write.** `/activate` and `/start` don't exist; `PATCH .../update {"status":"active"}` is silently ignored. **`PATCH /api/campaigns/{id}/resume` is the universal activation** (draft to active, paused to active).
- Resume prerequisites, all four or you get an opaque "This campaign is incomplete": (1) a schedule, (2) at least one sequence step, (3) leads attached, (4) senders attached.
- `completed` campaigns reject direct resume: pause first, then resume.
- Post-resume `queued`/`launching` are normal transitional states for 10-30 seconds; re-check until `active`.
- `PATCH /api/campaigns/{id}/pause`: always safe, always reversible, the correct protective action while you think.
- `GET /api/campaigns/{id}` returns the full campaign including `sequence_id` (you need this for sequence writes) and `sender_emails_count`.

## Sequence steps: the trap field

Read: `GET /api/campaigns/{id}/sequence-steps` (unpaginated, see above).

Create: `POST /api/campaigns/{campaign_id}/sequence-steps` with `{"title": "...", "sequence_steps": [...]}`. All variants of a step go in ONE call: first item is the primary, every subsequent item carries `"variant": true` (and `variant_from_step_id` pointing at the parent when adding to an existing step).

The traps, each verified live:

1. **Reads return `variant_from_step`; writes require `variant_from_step_id`.** Map the field name or your write silently does the wrong thing.
2. **`PUT /api/campaigns/sequence-steps/{SEQUENCE_ID}` takes the CAMPAIGN's `sequence_id`** (from `GET /api/campaigns/{id}`), not a step id. A step id in that URL is the mystery 404.
3. **The PUT body needs top-level `title` plus all 9 fields per step**: `id, order, email_subject, email_body, wait_in_days, active, variant, variant_from_step_id, thread_reply`. Missing any produces a silent 500.
4. **PUT silently orphans variants.** Editing a variant via PUT returns 200 but clears its parent link, detaching it into a phantom sequential step (passing `variant_from_step_id` in the PUT body does not save it). **Edit variant copy by DELETE + re-POST, never PUT.** Only the non-variant parent step is safe to PUT-edit. After any variant work, re-read and assert every variant still links to its parent.
5. **PUT prepends `Re: ` to `thread_reply` subjects.** Send bare subjects or they compound to `Re: Re:`. Re-fetch and diff subjects after every write.
6. **A variant that has already sent cannot be deleted** (422, stats are preserved). To retire live copy: POST the corrected replacement variant, PAUSE the campaign (you cannot toggle a variant while it runs), deactivate the old one via `PATCH /api/campaigns/sequence-steps/{id}/activate-or-deactivate` with `{"active": false}` (the field is required), then RESUME. Re-read after resume and confirm `plain_text` survived.

Telling steps and variants apart in reads: incrementing `order` = sequential steps (every lead gets all of them); `"variant": true` = A/B variants inside a step (each lead gets one at random). Never upload variants as extra steps or extra campaigns.

## Leads

- **Bulk upsert:** `POST /api/leads/create-or-update/multiple`, proven at ~140 per batch. It UPDATES matched leads in place rather than erroring, so count workspace totals before and after to know what actually happened.
- **Attach:** `POST /api/campaigns/{id}/leads/attach-leads {"lead_ids": [...]}`, batches of 500, idempotent.
- **Detach:** `DELETE /api/campaigns/{id}/leads` with `{"lead_ids": [...]}` in the BODY. (`/remove-leads` and `/detach-leads` variants all 404.) Detaching only unlinks; the workspace lead record survives.
- **Bounced leads refuse attach** (422 "previously bounced"). The platform remembers; filter them out first, don't fight it.
- Leads already `in_sequence` elsewhere need a move (`target_campaign_id`), which is async and half-completing: poll the destination count until it lands, only then detach from source.
- **`custom_variables` must be an array of `{name, value}` objects.** A flat map 422s:

```json
"custom_variables": [
  {"name": "COMPANY_NAME", "value": "Acme"},
  {"name": "CITY", "value": "London"}
]
```

- Names UPPERCASE on write; the API lowercases them in responses (normal, match case-insensitively on read). **Never create a custom variable named `COMPANY`**: it collides with the native merge tag for the lead's company field.
- Scale numbers, verified at 50k: concurrency 10 + 500ms batch delay + backoff on 429 gives ~12 leads/sec with zero failures. Concurrency 40 gave a 63% failure rate.
- The platform's `unverified` flag on leads is meaningless; EmailBison verifies nothing on upload.
- `GET /api/campaigns/{id}/leads` ignores most filter params (`status`, `folder` return the full set) but `?search=` works, and cursor pagination walks the whole set past the page cap.

## Senders

- Workspace list: `GET /api/sender-emails?page=N` (paginate; `meta.total` is the true count).
- Attach: `POST /api/campaigns/{id}/attach-sender-emails {"sender_email_ids": [...]}`.
- **Zero senders attached = the campaign silently sends nothing.** Freshly created campaigns have zero. Verify with a full match, workspace list vs campaign's attached list, not just "more than none".
- Real capacity = the sum of `daily_limit` across CONNECTED attached senders.

## Stats

- `POST /api/campaigns/{id}/stats`. It is a POST, not a GET.
- `GET /api/campaigns/{id}/line-area-chart-stats?start_date=&end_date=` (both params required) is **the only true daily-volume read**. Cumulative `emails_sent` cannot tell you what sent today.
- `GET /api/campaigns/{id}/replies?folder=bounced` is where bounce reasons live (see `/deliverability-triage`).

## Schedule

`POST /api/campaigns/{id}/schedule`, one request, flat day booleans:

```json
{
  "monday": true, "tuesday": true, "wednesday": true, "thursday": true,
  "friday": true, "saturday": false, "sunday": false,
  "start_time": "09:00", "end_time": "17:00",
  "timezone": "America/New_York",
  "save_as_template": false
}
```

`save_as_template` is REQUIRED (omitting it errors); send `false` unless you genuinely want a template. The schedule's `status` field is unreliable: it can read "Not Started" indefinitely while the campaign sends at full volume. Pre-launch, verify the schedule EXISTS with the right days/hours/timezone; post-launch, `emails_sent > 0` within the window is the real proof of life. Never pause/resume/re-POST a live campaign because the label says "Not Started".

## Variables in copy

- `{{DOUBLE_CURLY_UPPERCASE}}` throughout.
- Native tags: `{{FIRST_NAME}}`, `{{COMPANY}}`, `{{SENDER_FIRST_NAME}}`.
- `<br>` for line breaks in HTML bodies (only relevant when `plain_text: false`).

## Common gotchas (the compressed list)

1. Exactly-15 result count: you read page 1 only.
2. `data` is an object, not an array: auth error, not zero results.
3. Partial PATCH flipped your settings: re-send all booleans, then GET and diff.
4. "This campaign is incomplete" on resume: schedule, steps, leads, senders, one of the four is missing.
5. Sequence PUT 404: you passed a step id where the campaign's `sequence_id` goes.
6. Variants detached into phantom steps: a PUT orphaned them; DELETE + re-POST.
7. `Re: Re:` subjects: you sent a `Re:`-prefixed subject to a thread-reply step.
8. Campaign "live" but sending nothing: zero senders attached, or you trusted the schedule status label.
9. 422 on custom variables: you sent a flat map instead of `[{name, value}]`.
10. 422 on attach: bounced leads in the batch, filter first.

## What to do next

This is a reference; it doesn't do anything by itself. The build flow that uses it is `/emailbison-campaign-launch`, and nothing that flow produces goes live without `/pre-launch-qa`.

## Related skills

- `/emailbison-campaign-launch`: the build flow built on these endpoints
- `/pre-launch-qa`: the mandatory gate before any launch or resume
- `/deliverability-triage`: reading bounces via the replies endpoint
- `/sender-scaling`: what the sender daily limits should actually be
