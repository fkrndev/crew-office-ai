# 05: Domain and Inbox Setup (condensed walkthrough)

From nothing to warmed senders connected in EmailBison: Dynadot for domains, Zapmail for inboxes and DNS, then export into your sending workspace. This is the condensed version; `/domain-inbox-setup` has the full detail and gotchas. The flow is adapted from Eric Nowoslawski's open-source Zapmail setup guide (MIT licensed), rewritten for my stack.

Prerequisites: `DYNADOT_API_KEY` (API enabled, **your IP whitelisted**, wallet funded) and `ZAPMAIL_API_KEY` in `.env`. Run `npm run verify` first.

## The flow at a glance

```
1. Generate domain candidates       instant
2. Availability check + purchase    minutes (confirms cost first)
3. Nameservers to Zapmail           minutes, then WAIT 15-20 min for DNS
4. Connect domains on Zapmail       minutes, then WAIT 10-30 min to assignable
5. Create 2 inboxes per domain      minutes, then WAIT 4-6 hours to ACTIVE
6. Warmup                           2-3 WEEKS, non-negotiable
7. Export senders into EmailBison   minutes
```

The waits are the point. Every shortcut here gets paid back in spam-folder placement later.

## Step 1: generate candidates

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-generate-domains.ts
```

Produces lookalike variations of your brand in tiers: prefix + brand first (`goacme.com`, `tryacme.com`, `getacme.com`), brand + suffix second (`acmehq.com`, `acmelab.com`). Shorter is better; aim under 20 characters before the TLD. The script filters by length only, it doesn't catch an awkward word hiding inside a combined name, so read the generated list before buying.

How many? 10-20 domains is the standard fleet (2 inboxes each). Buy for the volume you'll want in a month, not today; at proven-stage rates (3/day per sender), 20 domains is ~120 sends/day.

## Step 2: check and buy

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/dynadot-bulk-purchase.ts
```

Batch-checks availability, filters to your price cap, prefers `.com` (~$12, best trust) with `.co` as the fallback, then **prints the exact total cost and waits for your explicit yes** before buying anything. Never let a script spend money without showing the bill; that confirm is permanent.

Avoid `.info`, `.xyz`, `.click`, `.top` and friends. Recipients and filters both pattern-match on cheap TLDs.

## Step 3: nameservers to Zapmail

The setup script (`zapmail-setup.ts`) switches the new domains to Zapmail's nameservers as its first phase. Then wait 15-20 minutes for DNS propagation before moving on; Zapmail rejects domains it can't see yet.

## Step 4: connect domains on Zapmail

```bash
npx tsx ${CLAUDE_PROJECT_DIR}/.claude/skills/domain-inbox-setup/scripts/zapmail-setup.ts
```

Connects domains in batches and polls until they're assignable (most land within 20-30 minutes). Proceed when >95% are assignable; retry stragglers individually later. Zapmail also sets SPF, DKIM, and DMARC on every domain automatically, which is most of why it's in the stack (see `01-deliverability-basics.md` for what those are).

## Step 5: two inboxes per domain

Same script, next phase. The standard identity pattern, one real-looking sender kept consistent across the fleet:

- `sarah@goacme.com`
- `sarahchen@goacme.com`

Two per domain is the ceiling; more hurts reputation. Zapmail returns success immediately but inboxes sit "In Progress" for 4-6 hours. Do not touch them until they read ACTIVE. One trap the script handles for you: a single already-existing mailbox fails an entire Zapmail batch, so failures retry per-domain.

## Step 6: warmup, 2-3 weeks

The inboxes go into warmup and stay there for 2-3 weeks before any real send. This is the single most-skipped step and the single most expensive one to skip. Warmup also never turns off afterwards; it runs underneath real sending forever.

Use the window: offer (`02`), copy (`03`), sequence (`04`), list (`06`). You should be launch-ready the day warmup finishes.

## Step 7: connect senders into EmailBison

Two routes, once inboxes show ACTIVE:

1. **Zapmail's export integration**, if EmailBison appears under Exports, connected accounts in your Zapmail dashboard: link the account and export, always filtered to `"status": "ACTIVE"`. Exporting In Progress inboxes fails silently and you'll wonder where your senders went.
2. **Credentials export + import** (always works): export the mailbox credentials from Zapmail (email, password, IMAP/SMTP hosts), then in EmailBison use Sender Emails, import to connect them in bulk.

Verify the landing on the EmailBison side: `GET /api/sender-emails`, paginating every page to `meta.last_page`, and confirm the count matches what you connected. If you see exactly 15, you read one page of a paginated list, not the total; EmailBison returns 15 per page always (see `/emailbison-api`).

## The gotchas that actually bite

1. **Dynadot IP whitelist.** Un-whitelisted IPs fail in unhelpful ways. Do it at signup.
2. **Fixed sleeps instead of polling.** DNS and assignability vary; the scripts poll, don't replace that with timers.
3. **Exporting before ACTIVE.** Silent empty export.
4. **Skipping warmup because "the inboxes work".** They send. Nobody receives.
5. **Full volume on day one post-warmup.** New senders start at 1 real email/day and earn 3/day on proven engagement. That discipline is `/sender-scaling` and it's not optional.
6. **Reusing a domain across two brands or identities.** Never. One domain, one identity.

## Next

`/domain-inbox-setup` for the full skill. While warmup runs: `06-list-building.md`.
