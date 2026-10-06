---
name: pre-launch-qa
description: "The mandatory quality gate before ANY campaign launch, resume, reactivation, or lead upload goes live: a numbered checklist covering copy (variables resolve, no links, spintax renders, subjects lowercase), settings (plain_text by type, tracking off, limits), leads (counts, hygiene, zero Mimecast/Barracuda), senders (all attached, connected, warmed), schedule, and sequence structure. Reported as PASS/FAIL per item. Triggers on 'launch', 'go live', 'resume the campaign', 'activate', 'turn it on', 'restart the campaign', 'is this ready to send', 'pre-launch check', 'QA the campaign'. The rule: no checklist, no launch."
---

# Pre-Launch QA: the gate

Every expensive cold email failure I've had shipped through a gap that a checklist would have caught: a campaign that sent with a broken variable in the subject line, a list that went out with security-gateway domains in it, settings that flipped on a live campaign because of a partial update. None of these were exotic. All of them were checkable in minutes.

So this is the rule, and it has no exceptions: **no checklist, no launch.** Not for new campaigns, not for resumes after a pause, not for reactivations after edits, not for "I just added a few leads". Any event that starts or restarts sending runs this gate first. The checklist takes ten minutes; each of the failures above cost days and burned domain reputation that took weeks to recover.

## When to use this

- Before pressing start on any new campaign
- Before resuming any paused campaign
- Before reactivating a campaign you've edited (new step, new variant, new leads)
- After any lead upload to a campaign that will send
- When anyone, including you, says "it's fine, just launch it"

That last one especially.

## How to run it

Walk every item below in order. Report each one as **PASS** or **FAIL** with one line of evidence (the actual count, the actual setting value, the actual rendered output). No "looks fine". A FAIL stops the launch until fixed and re-checked. When all items pass, present the full PASS/FAIL table and let the human press start; this skill never activates anything itself.

All reads use the mechanics in `/emailbison-api`. In particular: paginate everything to `meta.last_page`, use cursor pagination for big lead sets, and treat an exactly-15 count as an unpaginated read.

---

## 1. COPY

- [ ] **1.1 Every variable resolves on every lead.** Extract every `{{VAR}}` used across all steps and all variants. While paginating the attached leads, confirm each variable resolves for every lead (native fields plus `custom_variables`). One lead with a missing `{{COMPANY_NAME}}` sends "Hi {{COMPANY_NAME}}" to a real person. Zero unresolvable = PASS.
- [ ] **1.2 No links, no URLs, no dotted domains anywhere in cold copy.** No `https://`, no `www`, no `acme.com`. Write domains with a dash if you must reference one ("acme-com"). Links are a primary spam signal in cold email.
- [ ] **1.3 All spintax renders.** Braces balanced, `|` alternatives present in every group, no spintax wrapping a `{{VAR}}` token. Render-test at least one fully resolved output per step and read it as a human: every path must be a sentence someone could have typed.
- [ ] **1.4 Subjects are lowercase** and consistent in style across variants of the same step. A capitalised, headline-styled subject reads as marketing; lowercase reads as a colleague.
- [ ] **1.5 Variants are variants.** Check the reads: A/B variants carry `variant: true` inside one step; sequential steps have incrementing `order`. Variants uploaded as steps means every lead gets all of them, days apart.
- [ ] **1.6 Exactly one opt-out line, at the bottom, in every step and every variant.** Present, last line, and counted once. Two opt-out lines (one baked into copy plus one appended) is a real and common failure.
- [ ] **1.7 No stale time-bound framing.** "Ahead of the new year" in a campaign resuming in March.

## 2. SETTINGS

- [ ] **2.1 `plain_text` matches the campaign type.** `outbound` with plain-text copy: `true`. `reply_followup`: **always `false`** (its bodies carry an HTML calendar link; plain text kills it). Read the type off the campaign, don't assume.
- [ ] **2.2 `open_tracking: false`.** Tracking pixels hurt deliverability and opens are vanity data.
- [ ] **2.3 `include_auto_replies_in_stats: false`.** Out-of-office noise corrupts the reply rate you steer by.
- [ ] **2.4 Daily limits 1000+.** Senders govern real volume; a low campaign cap is a silent throttle.
- [ ] **2.5 Verified by GET-and-diff**, not by memory of what you set. Partial PATCHes reset unspecified booleans; the only trustworthy read is the one you just did.

## 3. LEADS

- [ ] **3.1 Count matches expectation.** Campaign `total_leads` equals the list you meant to upload. Enumerate the attached set with cursor pagination and assert `fetched == meta.total`; a short read voids every check below it.
- [ ] **3.2 Hygiene done.** The list has been through `/list-hygiene`: deduped, verified where it needed verifying (lists you brought yourself; AI Ark exports ship pre-verified, see `/email-verification`), invalids filtered out.
- [ ] **3.3 ZERO Mimecast or Barracuda domains in the send set. Hard rule, no exceptions.** MX-check EVERY unique domain in the attached list, never a sample (a 40-domain spot-check has come back clean on a list where the full sweep found hits; a full sweep is parallel `dig MX` and takes minutes even at thousands of domains). These security gateways reject new-domain cold mail on policy, not on content, so there's nothing to weigh here: any domain resolving to `*.mimecast.*` or containing `barracuda` gets detached before launch (`DELETE /api/campaigns/{id}/leads` with the ids in the body; detaching destroys nothing). `/list-hygiene` does this automatically upstream; this check just confirms it actually ran.
- [ ] **3.4 No bounced-status leads attached.** The platform refuses to send to them anyway; their presence means the filter upstream is broken.

## 4. SENDERS

- [ ] **4.1 ALL workspace senders attached.** Paginate both sides (workspace list and campaign's attached list) and require a full match, not just "some". Zero senders sends nothing, silently.
- [ ] **4.2 All attached senders show Connected.** A handful disconnected is normal churn; if it's more than roughly 10, stop and investigate before launch (see `/deliverability-triage`).
- [ ] **4.3 Senders are warmed.** 2-3 weeks of warmup before real volume, warmup still enabled, and per-sender daily limits set per `/sender-scaling` (1/day for unproven senders).

## 5. SCHEDULE

- [ ] **5.1 A schedule exists** for the campaign. It's a hard prerequisite; resume fails opaquely without one.
- [ ] **5.2 Days and hours are sane**: weekdays, business hours.
- [ ] **5.3 Timezone matches the RECIPIENTS' geography**, not yours. A US list on a London timezone sends at 4am Eastern.
- [ ] Note: the schedule's `status` label is unreliable and can read "Not Started" while sending works fine. Check existence and correctness here; after launch, `emails_sent > 0` inside the window is the real proof of life.

## 6. SEQUENCE

- [ ] **6.1 Step gaps are 3+ days**, every `wait_in_days` between follow-up steps. Faster than that reads as harassment and burns the list.
- [ ] **6.2 Thread structure is deliberate.** Default architecture: step 2 at +3 days in the same thread (`thread_reply: true`, bare subject; the platform adds `Re:` itself); step 3 at +30 days as a NEW thread with a different offer; step 4 at +3 days in step 3's thread. Deviations are allowed but must be on purpose (see `/sequence-architecture`).
- [ ] **6.3 Every variant links to its parent step.** Orphaned variants (a cleared parent link, usually from a PUT edit) silently become sequential steps. Re-read the sequence and assert the links.

## 7. ACTIVATE + WATCH (after the human presses start)

- [ ] **7.1 Resume prerequisites present**: schedule, at least one step, leads, senders. Missing any gives an opaque "This campaign is incomplete".
- [ ] **7.2 The human activates.** Via the UI or `resume`, by their own hand. This checklist never launches.
- [ ] **7.3 Status settles to active** (brief `queued`/`launching` is normal).
- [ ] **7.4 First-sends watch:** within the first hour, pull the bounced replies and read the actual bounce text. Over 5% bounce with 25+ sends: pause immediately (always safe, always reversible) and run `/deliverability-triage`.

---

## Common gotchas

1. **Running the checklist on a sample.** The variable check and the MX sweep are only valid on the FULL attached set, cursor-paginated, `fetched == meta.total` asserted.
2. **"It passed last week."** A resume after edits is a new launch. Settings drift via partial PATCHes; leads get added; re-run the gate.
3. **Trusting the schedule status label** in either direction.
4. **Client-supplied or hand-edited copy skipping the checks.** Copy that bypassed your copywriting process is the copy most likely to carry a spam word or a link. The gate applies regardless of who wrote it.
5. **Treating a FAIL as advisory.** A FAIL is a stop. Fix, re-check, then proceed.

## What to do next

All items PASS: press start yourself, then do the 7.4 first-sends watch and pull early numbers with `/emailbison-campaign-launch`'s analytics script. Any item FAILED: fix it with the relevant skill (`/list-hygiene` for lead failures, `/cold-email-copywriting` for copy, `/sender-scaling` for sender readiness) and run this gate again from the top of the affected section.

## Related skills

- `/emailbison-campaign-launch`: the build flow this gate sits behind
- `/emailbison-api`: pagination and read mechanics every check depends on
- `/list-hygiene`: fixes for section 3 failures
- `/deliverability-triage`: where the first-sends watch escalates
- `/sequence-architecture`: the structure section 6 checks against
- `/sender-scaling`: sender readiness and limits
