# 00: Getting Started

Start here if you've never sent a cold email. This file covers whether this system is for you at all, the accounts you need, what it costs, the `.env` setup, and what a realistic first 30 days looks like.

## Is this for you? (answer honestly)

**1. Are you selling to businesses?** This entire system is B2B. If your buyers are consumers, stop here. Cold emailing consumers is a legal minefield and this kit does not cover it, on purpose.

**2. Do you have the budget?** Roughly $300-500 for month one (exact table below, all approximate). If that's painful, halve the domain count and go slower. Do NOT economize by skipping warmup, hygiene, or verification of outside lists; those "savings" cost you the whole operation.

**3. Do you have the time reality straight?** Cold email is not a this-week channel. Warmup alone is 2-3 weeks. Meaningful data takes ~6 weeks. If you need meetings in 10 days, this is the wrong tool for that deadline (though it's the right tool to start now for next quarter).

**4. Do you have an offer, or the willingness to build one?** The system's core belief: the one who wins cold email is the one who gives away the most genuine value for free, upfront. If your plan is "blast our pricing page at 50,000 people", the market will grade that plan quickly and harshly.

Yes to all four? Keep going.

## Accounts to create, starting today

Three of these have real lead time. Kick all three off in parallel on day one, before you write a word of strategy: EmailBison approval isn't instant, and warmup is 2-3 weeks no matter when you start it. Everything downstream of them can happen while they cook.

### EmailBison (sending) - required, start this FIRST
The sending platform. Invite-only, no free trial: request access at [emailbison.com](https://emailbison.com) and expect a wait before you're in, so don't leave this until you're otherwise ready. Pricing is a flat $599/mo for 500,000 emails, no per-seat fees. It's white-labeled, so your instance lives at whatever domain you're given on approval; that URL is your `EMAILBISON_BASE_URL`. Once in, create an API token in your workspace settings. Note: API keys are scoped per workspace, and workspaces can only be created in the UI, not the API.

If $599/mo is too much to commit before you've tested the system, this repo's copy, offer, and list-building skills all work independently of the sending platform; you'd just swap the EmailBison-specific skills and scripts for whatever you use instead.

### Domains and inboxes (Dynadot + Zapmail) - required unless you have warmed senders, start this FIRST too
Your longest lead time item, full stop: 2-3 weeks of mandatory warmup before anything real can send, and no way to compress it. Start `/domain-inbox-setup` on day one.

- **Dynadot** ([dynadot.com](https://www.dynadot.com)), domain registrar with a workable API. After signup: fund your wallet (the purchase script checks the balance but can't top it up), enable API access under Tools, and **whitelist your current IP address**. Un-whitelisted IPs get refused and the errors are not always obvious.
- **Zapmail** ([zapmail.ai](https://zapmail.ai)), provisions the actual sending inboxes on your domains and handles SPF/DKIM/DMARC automatically, which is most of the reason I use it. API key is in settings.

### AI Ark (list building) - required
The B2B contact database every list in this system comes from. Sign up free at [ai-ark.com](https://ai-ark.com): the free tier gives 100 credits/month and previews up to 25 records, enough to test the skill before you pay. Find the developer portal and generate an API key. Paid plans start at $49/mo for 5,000 credits; **exports cost about 1 credit per record** (roughly a cent a lead), previews are free.

### Kitt (verification) - optional
Only needed if you'll import lists from OUTSIDE AI Ark (scraped, bought, CRM exports). AI Ark exports are verified at source and never get re-verified; that rule is absolute in this system. Skip Kitt entirely until the day you bring your own list.

## What it costs (approximate, check current pricing)

| Item | Approx | When |
|---|---|---|
| 20 x .com domains (Dynadot) | ~$12 each, ~$240 | Annual, up front |
| Zapmail, 40 inboxes | ~$60/mo | Monthly |
| EmailBison subscription | $599/mo flat, 500k emails included | Monthly |
| AI Ark credits | free tier: 100 credits/mo; paid from $49/mo for 5,000 | As needed; previews free |
| Kitt credits | optional | Only for outside lists |

Two honest notes. First, the domain spend scales down cleanly: 10 domains and 20 inboxes is a perfectly good start at half the cost, you'll just test more slowly. Second, the biggest cost of cold email is never the tools, it's the weeks you lose to skipped steps. Budget patience.

## Set up the repo and `.env`

From the repo root:

```bash
npm install
bash .claude/skills/cold-email-starter-kit/scripts/setup-env.sh
```

The script copies `.env.example` to `.env` (never overwriting an existing one) and tells you where each key lives. Or do it manually:

```bash
cp .env.example .env
```

Open `.env` in your editor. It's sectioned by service with signup URLs and click-paths in the comments. Fill in only the keys for the skills you'll use:

- `AI_ARK_API_KEY`
- `EMAILBISON_BASE_URL` (your instance URL, no trailing slash) and `EMAILBISON_API_KEY`
- `DYNADOT_API_KEY` and `ZAPMAIL_API_KEY` (skip if you have inboxes)
- `KITT_API_KEY` (skip unless importing outside lists)
- The `SENDER_*` identity fields, including a real physical mailing address (legally required in cold email footers)

`.env` is gitignored and stays that way. Keys never go in flags, code, or chat.

## Verify everything

```bash
npm run verify
```

This pings every configured API with a cheap, safe call and prints a table. Core services (AI Ark, EmailBison) fail the run if missing or broken; infrastructure and verification keys just warn when absent. Fix anything marked failed before proceeding:

- **AI Ark failed:** key rejected, regenerate it in the developer portal.
- **EmailBison failed:** check both the base URL (your instance, no trailing slash) and the token. A wrong key on EmailBison can look like an empty success rather than an error, which is exactly why the verifier exists.
- **Dynadot failed:** almost always the IP whitelist.
- **Zapmail failed:** re-copy the token from settings.

All green (or green plus intentional skips)? You're wired up.

## What success looks like: the first 30 days

Calibrate expectations now, because week two is where people panic and break things.

**Weeks 1-3: warmup.** You buy domains and provision inboxes on day one (`05-domain-inbox-setup.md`), then the inboxes warm for 2-3 weeks. Zero real sends. This is not dead time: it's when you do the offer work (`02-offers-and-strategy.md`), write the copy (`03-copywriting.md`), structure the sequence (`04-sequence-structure.md`), and build the list (`06-list-building.md`). Done right, you are fully launch-ready the day warmup completes, instead of starting the thinking then.

**Weeks 3-4: first real sends.** Senders come out of warmup at 1 real email per day each. 40 senders means ~40 sends/day: small, deliberate, low-risk. Run `/pre-launch-qa`, press start yourself, and watch the first replies. Do not judge the offer yet; the sample is tiny.

**Weeks 4-6: ramp and read.** Senders that serve a campaign with proven engagement earn 3/day. Replies start arriving in enough volume to classify. The number that matters is the **interested reply rate**: 20%+ of replies interested is great, 10%+ is good, under 5% means the offer or list is wrong and more volume won't fix it.

**Week 6: meaningful data.** By now you have a real read on angle number one, and (because you built the testing roadmap in `02`) angle number two is already written. This is the rhythm from here on: test, read, swap the angle, scale what wins.

If someone promises you booked meetings in week one, they are either lying or burning domains they don't care about. Both are avoidable.

## Next

Read `01-deliverability-basics.md`. It's the "why" behind every rule in this kit, and it's short.
