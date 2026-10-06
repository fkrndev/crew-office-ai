---
name: domain-inbox-setup
description: "Set up cold email sending infrastructure end to end: generate short domain variations of your brand, check availability and buy them on Dynadot (.com first, .co fallback), switch nameservers to Zapmail, create two inboxes per domain, sit through warmup, then connect the senders into EmailBison. Triggers on 'set up domains', 'buy sending domains', 'domain setup', 'new inboxes', 'email accounts for cold email', 'sending infrastructure', 'secondary domains', 'Zapmail', 'Dynadot', 'I need senders'. Requires your own Dynadot and Zapmail API keys. Anything that spends money asks you first."
---

# Domain + Inbox Setup: Dynadot, Zapmail, EmailBison

Never send cold email from your main domain. One spam complaint on the domain your invoices and support emails live on and you have a real problem. Cold email runs on cheap, disposable secondary domains that look like your brand, warm up properly, and can be burned and replaced if they ever get stained.

This skill takes you from nothing to warmed inboxes connected in EmailBison. The Dynadot + Zapmail flow is adapted from Eric Nowoslawski's open-source Zapmail setup guide (MIT licensed), rewritten for my stack and with the senders landing in EmailBison instead of Smartlead.

## When to use this

- You're starting cold email from zero and have no sending infrastructure
- You're adding volume and need more domains and inboxes
- A domain got burned and you're replacing it
- Someone says "set up my domains" or "I need more senders"

## What you need first

Two API keys, stored in `.env` (never committed):

```bash
DYNADOT_API_KEY=your_key    # dynadot.com > Tools > API. Enable it, whitelist your IP.
ZAPMAIL_API_KEY=your_key    # zapmail.ai > Settings > API.
```

Dynadot API calls fail silently-ish if your IP isn't whitelisted, so do that bit properly. Fund your Dynadot wallet before you start; the purchase script checks the balance but it can't top it up for you.

## The full flow

```
1. Generate domain candidates          instant
2. Availability check + purchase       minutes (CONFIRMS COST FIRST)
3. Switch nameservers to Zapmail       minutes, then WAIT 15-20 min for DNS
4. Connect domains on Zapmail          minutes, then WAIT 10-30 min to become assignable
5. Create 2 inboxes per domain         minutes, then WAIT 4-6 HOURS to provision
6. Warmup                              2-3 WEEKS. Non-negotiable.
7. Connect senders into EmailBison     minutes
```

The waits are the point. Every shortcut here gets paid back in spam-folder placement later.

### Step 1: Generate short domain variations of your brand

Run `${CLAUDE_SKILL_DIR}/scripts/dynadot-generate-domains.ts` with your brand keyword. It produces candidates in two tiers:

- Tier 1, prefix + brand (best): `goacme.com`, `tryacme.com`, `getacme.com`, `myacme.com`
- Tier 2, brand + suffix: `acmehq.com`, `acmehub.com`, `acmelab.com`

Shorter is better, for deliverability and because recipients pattern-match on domains. The script filters to under 20 characters before the TLD, but doesn't screen for awkward substrings or unintended words hiding inside a combined name (`acmelab.com` reads fine; some prefix/suffix/brand combinations won't). Read the generated list yourself before buying.

How many domains? Most people starting out need 5-15. With 2 inboxes per domain and each inbox sending 3/day once proven (see `/sender-scaling`), 10 domains gets you to roughly 60 sends/day at safe volume. Buy for the volume you'll need in a month, not today.

### Step 2: Check availability and buy

Run `${CLAUDE_SKILL_DIR}/scripts/dynadot-bulk-purchase.ts` with your candidate list. It:

1. Batch-checks availability on Dynadot (100 per request)
2. Filters to available domains under your price cap
3. Prefers `.com` (best trust, usually $10-14), falls back to `.co` when the `.com` is taken
4. **Prints the exact total cost and waits for your explicit yes before buying anything**

That confirm step is deliberate and I won't remove it. Never let a script spend money without showing you the bill first. Avoid `.info`, `.xyz`, `.click`, `.top` and friends: recipients and filters both pattern-match on cheap TLDs.

### Step 3: Switch nameservers to Zapmail

The setup script (`zapmail-setup.ts`, phase 1) points every new domain at Zapmail's nameservers via Dynadot's `set_ns` command, batched 100 at a time.

Then wait 15-20 minutes for DNS propagation. Don't push on: Zapmail will just reject domains it can't see yet.

### Step 4: Connect domains on Zapmail

Run `${CLAUDE_SKILL_DIR}/scripts/zapmail-setup.ts`. It connects your domains (batches of 50), then polls the assignable endpoint every 5 minutes. Typical curve: ~8% assignable at 10 minutes, most by 20-30 minutes. Proceed when >95% are assignable; retry the stragglers individually later.

### Step 5: Create 2 inboxes per domain

Same script, next phase. Two inboxes per domain is the standard:

- `sarah@goacme.com` (first name)
- `sarahchen@goacme.com` (first + last)

Use a real-looking sender identity and keep it consistent across all inboxes. Zapmail returns 200 immediately but the inboxes show "In Progress" for 4-6 hours. Do not touch them until they read ACTIVE.

One nasty trap: if any single mailbox in a batch already exists, Zapmail fails the entire batch with a 400. The script handles this by retrying failed domains one at a time.

### Step 6: Warmup, 2-3 weeks, no exceptions

Zapmail-provisioned inboxes go into warmup. Leave them there for 2-3 weeks before real cold volume touches them. This is the single most-skipped step and the single most expensive one to skip: an under-warmed inbox lands in spam, the domain's reputation gets stained, and no amount of copy fixes that.

Warmup stays on even after you start sending real volume. It is background reputation maintenance, not a launch phase.

Use the wait. Build your list (`/ai-ark-list-builder`), write your copy (`/cold-email-copywriting`), frame your offer (`/offer-framing`). You should be launch-ready the day warmup finishes.

### Step 7: Connect the senders into EmailBison

Once inboxes are ACTIVE and warmed, connect them to EmailBison. Two routes:

1. **Zapmail's export integration**, if EmailBison appears under Exports > connected accounts in your Zapmail dashboard: link the account, export with `"status": "ACTIVE"` only. Exporting In Progress inboxes fails silently and you'll wonder where your senders went.
2. **Credentials export + import** (always works): export the mailbox credentials from Zapmail (email, password, IMAP/SMTP hosts), then in EmailBison use Sender Emails > import to connect them in bulk.

Verify the landing either way: `GET /api/sender-emails` on EmailBison, paginate to `meta.last_page`, and confirm the count matches what you connected. If you see exactly 15, you read one page of a paginated list, not the total (see `/emailbison-api`).

## Common gotchas

1. **Dynadot comma encoding.** URL-encoding turns commas into `%2C` and breaks batch nameserver requests. The scripts handle it; if you roll your own, keep commas literal in the `domain` parameter.
2. **Batch inbox failures.** One existing mailbox fails the whole Zapmail batch. Retry per-domain.
3. **Exporting too early.** In Progress inboxes export as nothing, silently. Poll for ACTIVE.
4. **Fixed sleeps instead of polling.** DNS and assignability vary. Poll the assignable endpoint rather than trusting a timer.
5. **Skipping warmup because "the inboxes work".** They work in the sense that they send. They don't work in the sense that anyone receives. 2-3 weeks.
6. **Reusing a domain across two brands or senders.** Never. One domain, one identity.
7. **Sending real volume at full tilt on day one post-warmup.** New senders start at 1 email/day. See `/sender-scaling`.

## What to do next

Warmup is your build window. Run `/icp-builder` if you haven't defined who you're targeting, then `/ai-ark-list-builder` for the list and `/cold-email-copywriting` for the copy. When the inboxes are warm and connected, `/emailbison-campaign-launch` assembles the campaign and `/pre-launch-qa` gates it before anything goes live.

## Related skills

- `/sender-scaling`: how fast these new inboxes are allowed to ramp (1/day until proven)
- `/emailbison-campaign-launch`: build the campaign these senders will power
- `/emailbison-api`: the API reference for verifying senders actually landed
- `/pre-launch-qa`: the mandatory gate before any send
- `/cold-email-kickoff`: the orchestrator that usually routes you here
