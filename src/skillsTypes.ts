// Skill row types — verbatim from upstream src/main/skills.ts (the preload
// re-exports them; renderer code imports from there). The shapes are what
// `skills:local`/`skills:catalog` return on the wire.
export interface LocalSkill {
  id: string;
  name: string;
  description: string;
  /** Which CLI reads this directory. */
  provider: 'claude' | 'opencode' | 'codex';
  /** 'user' = global for the whole machine, 'project' = one repo, 'bundled' = ships with the app. */
  scope: 'user' | 'project' | 'bundled';
  path: string;
}

export interface CatalogSkill {
  name: string;
  description: string;
  url: string;
  category: string;
  /** The publisher, taken from the source URL's GitHub owner — anthropics,
   *  stripe, supabase. The names in this list are bare (`docx`, `pdf`), so the
   *  URL is the only place the publisher appears. */
  owner: string;
}
