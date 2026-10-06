# 10: Legal Compliance

Cold email is legal when done right. This doc is the plain-English version of "done right": what the main laws ask for, how this system already satisfies most of it, and the short checklist to run before your first campaign.

**This is not legal advice.** I run a cold email agency, not a law firm. This is a practical summary of publicly available guidance. If you're in a regulated space (healthcare, finance, legal), sending at serious scale, or ever on the receiving end of a formal complaint, talk to an actual lawyer.

---

## The stance that makes everything else easy: B2B only

This entire system is built for business-to-business outreach: work email addresses, at companies, about things relevant to the recipient's job. That one constraint does most of your legal work for you.

- **Never email consumer addresses.** No gmail.com, hotmail.com, yahoo.com personal accounts. This isn't just legal caution; consumer inboxes are also where deliverability goes to die.
- **Only email people whose role your offer is relevant to.** A VP of Sales about a sales tool: fine. The same VP about car insurance: not fine, and under EU rules genuinely not legal.
- **Skip .gov and .mil addresses.** Some agencies escalate to IT, and nothing good comes of it.

If your list building follows `/ai-ark-list-builder`, you're pulling work emails filtered by role and company already. The B2B stance is baked in.

---

## CAN-SPAM (USA)

The US law governing commercial email. The good news: it does NOT require prior consent for B2B or B2C, so cold email is legal. It asks for six things:

1. **Truthful sender identity.** The From name and address are a real person at a real organization. This system already demands it (real sender identity, consistent across inboxes, see `05-domain-inbox-setup.md`).
2. **Non-deceptive subject line.** Never fake a prior relationship. No "Re:" on a first touch (in this system, only genuine thread replies carry Re:, and the platform adds it itself).
3. **Identifiable as commercial.** Implicit is fine; your email obviously being outreach satisfies this.
4. **A physical mailing address** in the email. See below.
5. **A working opt-out mechanism.** See below.
6. **Opt-outs honored** within 10 business days, across everything you send.

Penalties are per email, so sloppiness at volume gets expensive fast. But every item above is either already doctrine in this system or a one-time setup.

---

## GDPR (EU and UK)

Stricter than CAN-SPAM, and the one people are most scared of. The short version for cold email:

**B2B cold email to EU and UK recipients is generally workable under "legitimate interest"**, one of GDPR's lawful bases for processing data. To rely on it honestly:

1. **Relevance is the whole game.** The email must be relevant to the recipient's professional role. This is the legal expression of good targeting: if your ICP work is honest, you're most of the way there.
2. **Easy opt-out, honored immediately.** Not in 10 days. Now.
3. **Write down your basis.** One internal paragraph: "We contact [role] at [type of company] about [category] under legitimate interest." Ten minutes, and it's the difference between a considered position and a shrug if anyone ever asks.
4. **Honor deletion requests.** If someone asks you to delete their data, do it, everywhere you hold it, within 30 days.
5. **Never email EU consumers.** Personal addresses are a hard no. The B2B stance covers you.

UK rules (UK GDPR plus PECR) land in practically the same place for B2B: corporate email addresses are workable with relevance and easy opt-out.

---

## CASL (Canada) and everywhere else

Canada's CASL is stricter: it wants consent, and for cold email you're relying on **implied consent**, which roughly means their business email is published (company site, professional profile) AND your message is relevant to their role. Both conditions, not one. If a Canadian recipient doesn't clearly qualify, skip them.

Australia's Spam Act works similarly to CASL (inferable consent from a published business contact plus relevance). Other jurisdictions vary. The honest summary: **the same three habits satisfy nearly everyone**: relevant B2B targeting, truthful identity, and instant opt-out handling. If you're sending at scale into a specific country, spend an hour reading its actual rules.

---

## Unsubscribes: the part you must get right

This is the one area where I want you paranoid.

**How opt-outs work in this system:** cold email carries no links (links are a spam signal, see the copy doctrine), so the opt-out in cold sends is a reply instruction, one plain line at the bottom of every email, present exactly once:

> ps: if this isn't relevant, reply and I'll stop emailing

The `/pre-launch-qa` gate literally checks for this: exactly one opt-out line, at the bottom, in every step and every variant. Two opt-out lines (one baked into copy plus one appended by the platform) is a real and common failure, so know which mechanism you're using.

**When someone opts out:**

1. **Honor it instantly.** Not within 10 business days because CAN-SPAM allows it. The same day, ideally the same hour. Legal minimums are floors, not targets.
2. **Add them to a suppression list** that every campaign checks, forever. Removing them from one campaign while another keeps sending is both illegal and humiliating. In EmailBison, unsubscribed leads are suppressed platform-wide; on top of that, keep your own master exclusion list and feed it into every list build (see `/list-hygiene`).
3. **Suppress the whole scenario, not just the address.** If someone says "stop emailing our company", stop emailing the company.
4. **Never respond to an unsubscribe with a pitch.** Silence or a one-line "done, sorry for the noise" and out.

Interpret opt-outs generously. "Not interested, please don't contact me again" is an unsubscribe even though it doesn't say the word.

---

## The physical address

CAN-SPAM (and CASL) require a physical mailing address in commercial email. Options:

- Your real business address, if you're comfortable publishing it
- A PO box
- A virtual office or mailbox service (cheap, and fine legally)

Set it once in `.env` (`SENDER_PHYSICAL_ADDRESS`) and put it in the signature block. It's one quiet line that satisfies multiple laws at once. Don't skip it because the emails "look cleaner" without it.

---

## The pre-launch legal checklist

Run this once before your first campaign, then again whenever your sender identity or footer changes:

- [ ] Sender name is a real person; From address matches the sending domain
- [ ] Subject lines don't fake familiarity (no manufactured "Re:", no "following up" on a first touch)
- [ ] Physical mailing address present in every email
- [ ] Exactly one opt-out line, bottom of every step and every variant
- [ ] Opt-out handling is wired: replies containing an opt-out get suppressed same-day, across all campaigns
- [ ] Master suppression list exists and every new list is checked against it
- [ ] List is 100% B2B work addresses, no consumer domains, no .gov/.mil
- [ ] Every recipient's role is genuinely relevant to the offer (if you're stretching this, fix the targeting, not the wording)
- [ ] Legitimate-interest note written down (one paragraph, internal)
- [ ] You have a way to delete a person's data everywhere if asked

All checked: you're in better shape than most of the industry. Anything unchecked: fix it before launch, it's cheaper than fixing it after.

---

## If you get an angry reply or a legal threat

Most "remove me or my lawyer will hear about this" replies are venting. Handle every one the same way: suppress them everywhere immediately, reply once, briefly and politely if a reply is warranted, and keep a record. If an actual lawyer or regulator contacts you, stop improvising and get your own advice.

---

## What to do next

Wire the checklist items into your setup now (address in `.env`, opt-out line in your copy templates, suppression list in your hygiene step), then move to `11-pre-launch-checklist.md` for the full go/no-go gate.
