---
name: marketplace-browser-ops
description: Operate marketplace seller centers (Shopee, TikTok Shop, Lazada) through the ShardX anti-detect browser. Use when the task needs Seller Center UI actions — replying to buyer chats, joining flash sales, reading dashboards, or anything the seller API does not expose. Requires the shardx MCP server enabled for this agent.
metadata:
  category: commerce
  domain: marketplace-ops
---

# Marketplace Browser Ops (ShardX)

You drive a real logged-in marketplace session through the ShardX launcher's
anti-detect profiles. Each profile is one account — its fingerprint, proxy,
cookies, and storage are stable across sessions, so the marketplace sees the
same "device" every time.

## When to use this skill

- Seller Center actions with no API equivalent (buyer chat replies, flash-sale
  enrollment, promotion toggles, live dashboards).
- Multi-account work: pick the profile bound to the right shop, never mix
  profiles across accounts.
- Visual verification: screenshot dashboards to confirm an API-side change.

## When NOT to use this skill

- Anything the seller API exposes — prefer `sellabot`-style API calls. APIs are
  faster, rate-limit-friendly, and don't break on DOM changes.
- First login — a human must open the profile once and complete OTP/captcha.
  If `browser_navigate` lands on a login wall, STOP and ask the human to finish
  login in the launcher; never type credentials yourself.

## Workflow

1. `list_profiles` — find the profile for the target account. The profile name
   is the contract: `shopee-toko-a`, `tiktok-shop-b`, etc. If the account you
   need has no profile, ask the human to create one — do NOT reuse another
   account's profile (linked-account bans are real).
2. `start_profile(id)` — returns the CDP endpoint. If already running, reuse it.
3. `browser_navigate(url)` to the Seller Center section you need.
4. Interact via `browser_click`, `browser_fill`, `browser_type`,
   `browser_evaluate`. Prefer `browser_get_text`/`browser_snapshot` before
   acting — marketplace SPAs re-render and stale selectors are the top failure.
5. Destructive writes (publish listing, confirm refund, join campaign): state
   the exact effect and confirm with the human first. Money moves on your word.

## Guardrails

- One profile per account, always. Two agents on the same profile is fine —
  the same account is the same identity.
- Keep actions paced like a human: a click storm looks like a bot even in a
  stealth browser.
- Captcha or unexpected OTP → hand off to the human; do not retry loops.
- Never export cookies or session data out of the profile.
