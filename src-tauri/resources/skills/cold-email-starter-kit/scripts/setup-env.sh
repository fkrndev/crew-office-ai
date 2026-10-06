#!/usr/bin/env bash
# Create your .env from the template, without ever overwriting an existing one.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../../.." && pwd)"

if [ -f "$ROOT/.env" ]; then
  echo "You already have a .env at $ROOT/.env, not touching it."
  exit 0
fi

cp "$ROOT/.env.example" "$ROOT/.env"
echo "Created $ROOT/.env"
echo
echo "Now open it and add your keys. Where to find each one:"
echo "  AI Ark      https://ai-ark.com        developer portal, API key"
echo "  EmailBison  https://emailbison.com    invite-only, request access EARLY (approval takes time)"
echo "  Dynadot     https://www.dynadot.com   Tools, API (only to buy domains)"
echo "  Zapmail     https://zapmail.ai        Settings, API keys (only to create inboxes)"
echo "  Kitt        https://trykitt.ai        dashboard, API (only for your own lists)"
echo
echo "EmailBison access and domain/inbox warmup both take time to kick in."
echo "Start those two today even if you're not ready to write copy yet."
echo
echo "Then run: npm run verify"
