# What `npx skills` covers

Status: research findings, 2026-11-05. Answers issue #3. Facts are checked against
primary sources (upstream repo docs and source on `main` at v1.7.6, the npm
registry, the GitHub API) and against the owner's own machine (read-only, treated
as data). No `GLOSSARY.md` exists in the repo yet, so terms follow `docs/goals.md`
and `docs/design-notes.md` (source, manifest, lock, adaptation, destination, profile).

## Identification

The installer is the `skills` npm package, run as `npx skills`, from
`vercel-labs/skills` (GitHub description: "The open agent skills tool - npx
skills"; homepage skills.sh).

- npm `skills@1.7.6`, bins `skills` and `add-skill`, repository
  `git+https://github.com/vercel-labs/skills.git`, maintainers `rauchg`, `quuu`
  (`npm view skills`).
- Verified match with the owner's lock: `~/.agents/.skill-lock.json` is `version: 3`
  with top-level keys `dismissed`, `lastSelectedAgents`, `skills`, which is the
  `SkillLockFile` interface in `src/skill-lock.ts` (CURRENT_VERSION = 3; path
  `~/.agents/.skill-lock.json`, or `$XDG_STATE_HOME/skills/.skill-lock.json` if set).
- The package ships both a CLI and a library (`import ... from 'skills'`).

## Commands (docs/commands.md, README.md)

| Need | Command |
| --- | --- |
| Install | `npx skills add <source> [-s skill...] [-a agent...] [-g] [--copy] [-y] [--all]` |
| Preview a source | `npx skills add <source> --list` |
| Check updates | `npx skills check` (exit code 1 when any update exists, usable as a CI gate) |
| Update | `npx skills update [skills...] [-g\|-p] [-y]` |
| List / remove | `npx skills list` (`ls`), `npx skills remove` (`rm`) |
| Search | `npx skills find [query] [--owner x]` (skills.sh directory) |
| One-shot, no install | `npx skills use <source>@<skill> [--agent a]` (prints a prompt, or launches an agent) |
| Scaffold | `npx skills init [name]` |
| Experimental | `experimental_sync` (skills shipped by npm deps and a `skills` field in `package.json`), `experimental_install` (restore from project `skills-lock.json`) |

Scope: project (default, `./<agent>/skills/`) or global (`-g`, `~/<agent>/skills/`).
Requires Node >= 22.20. Anonymous telemetry is on by default; disable with
`DISABLE_TELEMETRY=1` or `DO_NOT_TRACK=1`.

## Source formats

Accepted by `add` (docs/commands.md): GitHub shorthand `owner/repo` with an optional
`@ref` pin (release tag or full commit), GitHub/GitLab/Azure Repos URLs, tree URLs to
one skill folder, any git URL (https/ssh), local paths, "well-known" endpoints, and
direct URLs to a `SKILL.md` or `.zip/.tar/.tar.gz/.tgz` archive (default limits: 10 MiB
download, 25 MiB extracted, 1000 files). Private repos use existing git, `gh` or SSH
credentials.

## Source layouts understood (docs/creating-skills.md)

A skill is a directory with `SKILL.md` whose frontmatter has `name` and `description`.
Discovery searches the repo root, `skills/`, `skills/.curated/`, `.experimental/`,
`.system/`, and about 50 harness directories (`.claude/skills/`, `.agents/skills/`,
`.kiro/skills/`, ...), each walked up to 3 levels deep (flat, or one or two category
levels; a shallower `SKILL.md` shadows deeper ones). Also: skill paths declared in
`.claude-plugin/marketplace.json` / `plugin.json`; custom container dirs via
`skills.sh.json` `paths`; `--full-depth` and a recursive fallback search. On duplicate
names the first found wins. `metadata.internal: true` hides a skill unless
`INSTALL_INTERNAL_SKILLS=1`. This covers the layouts of the owner's sources
(`mattpocock/skills` uses `skills/<category>/<name>/`; `cursor/plugins` uses
`pstack/skills/<name>/`).

## Lock files

Two distinct formats.

**Global lock `~/.agents/.skill-lock.json`** (`src/skill-lock.ts`), keyed by skill name
under `skills`. Entry fields: `source` (normalized `owner/repo`), `sourceType`
(`github`, `local`, others), `sourceUrl`, optional `ref`, `skillPath` (path to
`SKILL.md` in the source), `skillFolderHash` (GitHub tree SHA of the skill folder;
changes if any file in it changes), `installedAt`, `updatedAt`, optional `pluginName`,
`sourceBaseUrl`, `wellKnownDigest`, `installMode: 'copy'`. The top level also holds UI
state (`dismissed`, `lastSelectedAgents`). Per the source, reading a lock with
`version` below 3 wipes it (treated as empty). It records no commit SHA of the source,
only the folder tree hash; `ref` is present only when one was given.

Owner's file: 104 entries (102 `github`, 2 `local`); sources `cursor/plugins` 58,
`mattpocock/skills` 35, `anthropics/skills` 3, `humanlayer/skills` 2, one each for
`vercel-labs/agent-browser` and `JuliusBrussee/caveman`. `pluginName` appears on the
mattpocock entries, `ref: main` on cursor/plugins entries.

**Project lock `skills-lock.json`** (`src/local-lock.ts`, version 1), meant to be
committed: entries have `source`, `sourceUrl`, `ref`, `sourceType`, `skillPath`,
`version`, `via`, `computedHash` (SHA-256 of folder contents on disk), `subagents`,
`wellKnownDigest`, `installMode`. Deliberately timestamp-free and sorted
alphabetically so branches merge cleanly.

## Harness directories and copy vs symlink

Per `docs/agents.md`, about 80 agents are supported. Many share the neutral
`.agents/skills/` (project) and `~/.agents/skills/` (global): Cline, Codex, Pi, Warp,
Zed and others. Others have their own dirs (Claude Code `.claude/skills/` and
`~/.claude/skills/`, Gemini `~/.gemini/skills/`, OpenCode
`~/.config/opencode/skills/`, Junie `.junie/skills/`). Several agents (Cursor, Gemini
CLI, OpenCode, Copilot) read `.agents/skills/` in project scope but use their own dir
globally.

Install modes (`src/installer.ts`, docs/commands.md): default **symlink** copies once
into the canonical dir `.agents/skills/<name>`, then symlinks each agent dir to it
(relative links); **copy** (`--copy`, recorded as `installMode: 'copy'`) writes an
independent copy per agent; an internal `link` mode symlinks the canonical dir to the
source. A failed symlink falls back to copy. `copyDirectory` dereferences symlinks
inside a skill and rejects ones pointing outside it. Project installs skip creating
links for agents whose config dir does not exist.

Owner's machine: `~/.agents/skills/` holds 112 dirs (the lock has 104 entries, so
about 8 are not tracked by the lock). `~/.claude/skills/` has 119 relative symlinks to
`../../.agents/skills/<name>` plus a `.trash` dir; `~/.junie/skills/` has 64 symlinks;
`~/.codex/skills/` has only `.system` (Codex reads `~/.agents/skills`). Consequence for
the sandbox goal: the default result is not self-contained. A read-only bind mount of
one harness dir would show dangling links unless `~/.agents/skills` is mounted too, or
copy mode is used.

## Programmatic use (docs/api.md)

Yes, first class. ESM-only library, Node >= 22.20, semver, public surface snapshot
checked in CI (tsnapi). Functions: `getSupportedAgents`, `detectAgents`,
`resolveSource` (fetch and list without installing; returns a disposable
`SkillSource`; `source.commit` is available for cloned git sources), `installSkills`,
`listSkills`, `removeSkills`, `version`. No prompts, no process exit; progress via
`onProgress`, warnings via `onWarning`; errors are typed diagnostics with permanent
`SKILLS_*` codes; per-skill failures go to `result.failed`. Options relevant here:
`agents: ['universal']` with `mode: 'copy'` and `lockfile: false` installs
materialized copies into `<cwd>/.agents/skills/<name>` (a plain staging dir);
`duplicates: 'error'`; `symlinks: 'reject'`; `ref` for pinning; and a
JSON-serializable `SkillsConfig` with agent allow/deny lists (the library never reads
config files itself). Telemetry can be disabled per call.

## Licence, cadence, activity

- Licence: MIT (repo LICENSE, GitHub API, npm). `ThirdPartyNoticeText.txt` is present.
- Created 2026-01-14; about 38.2k stars; last push 2026-11-05 (the day of research);
  249 commits since 2026-10-01 (GitHub commits API); 100 contributors listed;
  roughly 550 open issues and 250 open PRs.
- Releases: v1.5.20 (2026-07-22), v1.5.23 (08-18), v1.5.26 (09-11), v1.6.0 (09-16),
  v1.7.0 (09-17), v1.7.1 to v1.7.4 (10-06 to 10-19), v1.7.5 (11-02), v1.7.6 (11-03).
  Roughly weekly to biweekly. Marked a Vercel Labs product. Actively maintained and
  fast moving: the `sync` command became `experimental_sync` after 1.7.3, with `sync`
  kept as a compatibility alias.

## Update behaviour and local modifications

- Update detection compares the lock's `skillFolderHash` (GitHub tree SHA) with the
  latest tree of the source (`src/update.ts`). It asks only "did upstream change", not
  "did the installed copy change locally". There is no three-way comparison, no diff or
  review step, and no conflict handling.
- Update reinstalls the skill through the same path as `add`. `installer.ts`
  `cleanAndCreateDirectory` runs `rm -rf` on the target dir and recreates it from the
  fresh fetch. Local edits to an installed skill are overwritten silently. In symlink
  mode every harness sees the overwritten canonical copy.
- Updates follow the stored `ref` or the default branch tip. There is no rollback or
  history; `skills check` reports only that an update exists.
- No concept of adaptation, patches or transformation rules. The one harness-specific
  transform in the docs is Eve, which always receives copies because it rewrites
  frontmatter. `sourceType: local` entries are skipped by update checks.
- Entries lacking `skillFolderHash` or `skillPath` (older installs) cannot be updated
  automatically and are listed as legacy.
- Untrusted-content handling: archive size limits, unsafe symlinks not copied,
  metadata sanitized for terminal output. Installing does not execute skills.

## Known limitations relevant to skill-curator

1. No local-adaptation layer: updates overwrite, with no merge or review.
2. The lock records a folder tree hash and ref, not an immutable commit (a GitHub
   snapshot fetch may have no commit even through the API). Provenance is weaker than
   `docs/goals.md` asks for.
3. The global lock lives in the user's home and mixes machine UI state with source
   state, so it is not a portable manifest. The project `skills-lock.json` is portable
   but has no destination-profile concept, and `experimental_install` is experimental,
   creates no agent links, and cannot restore adapted content.
4. The default symlink layout conflicts with self-contained read-only sandbox mounts
   (workaround: `--copy` or `mode: 'copy'`).
5. No licence detection or attribution capture in lock entries.
6. Skill identity is the folder `name`, keyed globally by name: two sources with the
   same skill name collide in the lock and in the canonical dir.
7. Fast moving: experimental commands were renamed between minor versions. Telemetry
   is on by default.

## Implication for the open decision (drive or replace)

This is an observation, not a decision; the decision belongs to the map (issue #1).
`skills` is a capable fetch-and-place engine and can be driven as a library
(`resolveSource` then `installSkills` with `mode: 'copy'`, `lockfile: false`, `ref`,
`duplicates: 'error'`, `symlinks: 'reject'`) into a staging destination skill-curator
controls. It provides none of adaptation, review, three-way update, rollback, or
portable provenance. Driving it for fetch and discovery while owning manifest, lock,
adaptation and review looks feasible; relying on its own lock and `update` as the
update path is incompatible with the adaptation goals.

## Not verified

- Did not run `npx skills` or inspect the published tarball. Behaviour is taken from
  `main` source and docs at research time, which may differ slightly from 1.7.6.
- Issue and PR counts are approximate (`gh repo view` reported 549 open issues; the
  REST `open_issues_count` reported 798, which includes PRs; search showed 249 open PRs).
- Whether skills.sh is a hosted service with its own terms, plus the telemetry and
  security-audit endpoints, were not examined.
- Why about 8 skills in `~/.agents/skills` have no lock entry (manual creation or
  removed entries are likely, unconfirmed).
- Whether `update` of a copy-mode skill removes extra files in each agent dir. The
  copy path in `installer.ts` cleans the target dir first, which suggests yes.

## Sources

- https://github.com/vercel-labs/skills: README.md, docs/commands.md, docs/api.md,
  docs/agents.md, docs/creating-skills.md, docs/troubleshooting.md
- Same repo, `main`: `src/skill-lock.ts`, `src/local-lock.ts`, `src/installer.ts`,
  `src/update.ts`
- https://www.npmjs.com/package/skills (via `npm view skills`)
- GitHub API: releases, repo metadata, commits, search
- Local, read-only: `~/.agents/.skill-lock.json`, `~/.agents/skills/`,
  `~/.claude/skills/`, `~/.junie/skills/`, `~/.codex/skills/`
