# 12: Troubleshooting

Symptom, cause, fix. Start here when something breaks. Most of these I hit personally before writing them down, which is why the fixes are specific.

---

## The matrix

| Symptom | Likely cause | Fix |
|---|---|---|
| `npm run verify` shows AI Ark ❌ | Key missing, or wrong header (AI Ark wants `X-TOKEN`, not Bearer) | Re-copy the key from the developer portal into `AI_ARK_API_KEY`. If curling by hand, send `X-TOKEN` plus `Content-Type: application/json` on every call, GETs included |
| `npm run verify` shows EmailBison ❌ | Missing `EMAILBISON_BASE_URL` (there is no default, your instance has its own domain), trailing slash on it, or a key from the wrong workspace | Set both env vars; base URL with no trailing slash. Remember keys are scoped to ONE workspace |
| EmailBison returns 200 but the script says auth failed | That IS the auth failure. Bad keys return 200-shaped JSON with `data` as an object, not an array | Fix the key. Never read `data`-as-object as "zero results" |
| `npm run verify` shows Dynadot ❌ | API not enabled, or your IP isn't whitelisted (Dynadot is strict about this) | Dynadot > Tools > API: enable, then add your current IP to the whitelist. If your IP changes often, re-add before each run |
| `npm run verify` shows Zapmail ❌ | Wrong key, or wrong header name | Zapmail auth is the `x-auth-zapmail` header. Re-copy from Settings > API |
| `npm run verify` shows Kitt ❌ | Wrong key, or out of credits | Header is `x-api-key`. If configured but failing with funds-related text in the body, top up |
| AI Ark returns 401 on a search that has keyword excludes | The filter SHAPE is wrong, not your key. Company keyword excludes need the `sources` form: `{"any":{"exclude":{"sources":[{"mode":"WORD","source":"DESCRIPTION"}],"content":[...]}}}` | Use the shapes in `/ai-ark-api`. A 400 on the same filter means a bad `source` value (valid: NAME, KEYWORD, SEO, DESCRIPTION, INDUSTRY) |
| AI Ark search returns obviously unfiltered results | You used the flat `{any:{mode,content}}` filter form, which the API silently ignores | Every SearchMatch filter needs the nested `include`/`exclude` shape. Again, `/ai-ark-api` |
| AI Ark export stuck on PENDING | It's not stuck, it's processing. Exports are async and can take a while on big jobs | Be patient and keep polling the trackId every 10-30 seconds. The script does this for you |
| 409 while polling an AI Ark export | Still processing. A 409 here is not an error | Keep polling. Only FAILED state or a genuine HTTP error means something's wrong |
| AI Ark search returns 0 results out of nowhere | A typo'd enum value (seniority, department) fails silently with zero results, not an error | Check every enum string against `/ai-ark-api` before blaming the market |
| Every EmailBison list shows exactly 15 results | You read page 1 and stopped. 15/page is fixed and `per_page` is ignored | Paginate to `meta.last_page`, or trust `meta.total` from page 1. Exactly-15 is presumptively your bug |
| Campaign won't resume: "This campaign is incomplete" | One of the four prerequisites is missing: schedule, at least one sequence step, leads attached, senders attached | Check all four, fix the missing one, resume again. The error never tells you which |
| Campaign settings changed after an unrelated update | The PATCH boolean reset: `PATCH /update` resets every boolean you didn't send | Re-send the FULL boolean set on every settings change, then GET and diff. Settings never "drift" on their own |
| Leads uploaded but custom variables are missing or the upload 422s | `custom_variables` sent as a flat map instead of an array of `{name, value}` objects | Shape it as `[{"name":"CITY","value":"London"}]`, names UPPERCASE. And never name one `COMPANY` |
| Lead attach 422s | Bounced leads in the batch; the platform remembers and refuses them | Filter bounced-status leads out before attaching. Their presence means an upstream filter is broken |
| "Campaign is active but nothing is sending" | Zero senders attached, or you're outside the schedule window, or you trusted the schedule status label | Check `emails_sent`, attached sender count, and the schedule window. Labels lie; counts don't |
| High bounces | Several possible causes, and the fix depends entirely on which | Do NOT reflex re-verify. Run `/deliverability-triage`: pull actual bounce reasons, break down by ESP (the receiving mail provider: Gmail, Outlook, a corporate gateway) and SMTP code, then pick the one right lever |
| Volume going out, replies coming in, nobody interested | The offer, not the infrastructure. Deliverability problems look like silence; a bad offer looks like polite nos | Stop tweaking copy and run `/offer-framing`. Under 5% interested means offer or list; 0% with volume means pause first |

---

## Auth one-liners

Test each API's credentials by hand when the verify script isn't enough. All of these are read-only and free, except the Kitt one which burns one credit.

```bash
# AI Ark: returns your credit balance. Note X-TOKEN, not Bearer.
curl -s -H "X-TOKEN: $AI_ARK_API_KEY" -H "Content-Type: application/json" \
  "https://api.ai-ark.com/api/developer-portal/v1/payments/credits"

# EmailBison: data must be an ARRAY. An object here = auth failure, even on HTTP 200.
curl -s -H "Authorization: Bearer $EMAILBISON_API_KEY" -H "Accept: application/json" \
  "$EMAILBISON_BASE_URL/api/sender-emails?page=1"

# Dynadot: returns account info if the key works AND your IP is whitelisted.
curl -s "https://api.dynadot.com/api3.json?key=$DYNADOT_API_KEY&command=account_info"

# Zapmail: returns assignable domains (possibly an empty list, which is still a pass).
curl -s -H "x-auth-zapmail: $ZAPMAIL_API_KEY" \
  "https://api.zapmail.ai/api/v2/domains/assignable?limit=5&page=1"

# Kitt: verifies one address. COSTS ~1 CREDIT, so only run it deliberately.
curl -s -X POST "https://api.trykitt.ai/job/verify_email" \
  -H "x-api-key: $KITT_API_KEY" -H "Content-Type: application/json" \
  -d '{"email":"someone@example.com","realtime":true}'
```

Reading the results: any 401/403 is a bad or missing key (except AI Ark keyword filters, where a 401 can mean a malformed filter shape, see the matrix). EmailBison's failure mode is the sneaky one: it looks like success until you check the shape of `data`.

---

## The "where do I even start" flow

1. **Config problem?** (missing env var, ❌ in the verify table) Fix `.env`, re-run `npm run verify`.
2. **API behaving weirdly?** Check the matrix above, then the deep references: `/ai-ark-api` for list building, `/emailbison-api` for sending. Both document every trap I've hit live.
3. **Deliverability problem?** (bounces, spam folder, silence after healthy sending) `/deliverability-triage`, always. Diagnose before touching anything, and never reflex re-verify.
4. **Results problem?** (everything sends fine, nobody cares) The stack is fine, the offer isn't. `/offer-framing`, then `/reply-classification` to make sure you're reading replies honestly.
5. **Still stuck?** Re-read the numbered doc for the step you're on. The answer is usually a detail you skimmed the first time. I say that with love, because it's how I found half of these.

---

## What to do next

If the fix changed anything about a campaign (copy, leads, settings, senders), the campaign re-runs `/pre-launch-qa` before it resumes. A resume after edits is a new launch.
