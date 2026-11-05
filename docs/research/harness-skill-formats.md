# Skill format differences: Claude Code vs pi

Status: research findings for issue #4. Facts only; no design decisions.
Researched 2026-11-05 against Claude Code docs (live site) and pi 1.5.0
(`@earendil-works/pi-coding-agent`, installed locally and read-only).

Terminology: this repo has no `GLOSSARY.md` yet, so terms follow
`docs/goals.md` and `docs/design-notes.md`: **Skill**, **Source** (upstream
repository), **Adaptation** (copy/adapt/author), **Destination** (configured
install path), **Profile**.

## Sources

| Short name | Source |
|---|---|
| CC-skills | Claude Code skills reference, https://code.claude.com/docs/en/skills (fetched through a summarizing fetch tool; the last ~19k of 119k characters, the evaluation section, were not read) |
| AS-spec | Agent Skills specification, https://agentskills.io/specification |
| pi-skills | pi `docs/skills.md`, shipped in the package at `~/.pi/agent/install/releases/1.5.0/node_modules/@earendil-works/pi-coding-agent/docs/` |
| pi-src | pi loader source in the same package, `dist/core/skills.js`, `agent-session.js`, `package-manager.js`, `resource-loader.js` |
| pi-settings, pi-packages, pi-security | `docs/settings.md`, `docs/packages.md`, `docs/security.md` in the same package |

pi identification: `~/.pi/agent/bin/pi` launches a managed install of
`@earendil-works/pi-coding-agent` 1.5.0; repo `github.com/earendil-works/pi`
(`packages/coding-agent`) per its `package.json`. Docs and source were read
from the installed copy, not from GitHub.

## Summary

Both harnesses implement the Agent Skills layout (a directory with `SKILL.md`
plus optional `scripts/`, `references/`, `assets/`). The shared portable core
is `name` + `description`. Claude Code adds many frontmatter fields, a
substitution and shell-injection layer, and forked-subagent execution; pi
reads only `name`, `description`, and `disable-model-invocation`, and has none
of those features in its Skill format. They share `~/.agents/skills` and
project `.agents/skills` as search paths in pi, but pi does not read
`.claude/`, and Claude Code docs do not list `.agents/` or `.pi/`.

## 1. Frontmatter

| Field | Claude Code (CC-skills) | pi (pi-skills, pi-src) |
|---|---|---|
| `name` | Optional; defaults to directory name. Sets the `/` command. Names with control/bidi/zero-width chars or `<`/`>` are refused. Plugin skills are prefixed `/plugin:name`. | Optional in code (falls back to directory name). Validated against AS-spec rules (<=64 chars, lowercase, hyphens); violations are warnings and the skill still loads. Does not require a match with the directory name. Name collision: first discovered wins, warning. |
| `description` | Recommended, not required; falls back to first non-empty body line. Listing truncates `description`+`when_to_use` at 1,536 chars. | Required: a `SKILL.md` without a non-empty description is not loaded. Over 1024 chars is a warning. |
| `when_to_use` | Supported; appended to description. | Not read. |
| `disable-model-invocation` | `true` hides the skill from the model, blocks preload into subagents and scheduled tasks; user can still run `/name`. | Read (`=== true` strictly, so a YAML string does not count). Excluded from the system-prompt skill list; usable only via `/skill:name`. |
| `user-invocable` | `false` hides from `/` menu, Claude can still invoke. | Not read. No per-skill equivalent (global `enableSkillCommands` only). |
| `allowed-tools` | Pre-approves tools for the turn that invokes the skill (space/comma/YAML list). Permission grant, not a restriction. | Listed in pi-skills as a spec field ("experimental"), but the loader (`loadSkillFromFile`) does not read it; no pre-approval happens. Verified by source grep: no skill-level use. |
| `disallowed-tools` | Supported. | Not read. |
| `model`, `effort` | Per-turn model/effort override (also applied to a forked subagent). | Not read. |
| `context`, `agent`, `background` | `context: fork` runs the body in a subagent. | Not read. |
| `hooks` | Hooks registered while the skill is active. | Not read. (pi has extension hooks, but in TypeScript extensions, not in Skills.) |
| `paths` | Glob-gated auto-loading. | Not read. |
| `argument-hint`, `arguments` | Autocomplete hint and named positional args. | Not read. |
| `shell` | `bash` or `powershell` for inline shell injection. | Not read. |
| `license`, `compatibility`, `metadata` | Accepted, not acted on. | Accepted per pi-skills; not acted on. |

Unknown fields: Claude Code ignores them silently; pi's loader picks only the
fields above. If YAML does not parse, Claude Code still loads the skill with
no fields; pi does not load a malformed `SKILL.md`. Packaging for claude.ai
uploads and the Skills API accepts only `name`, `description`, `license`,
`compatibility`, `metadata`, `allowed-tools` and hard-errors on others
(CC-skills); relevant only if a Destination is the API.

## 2. Invocation

| Aspect | Claude Code | pi |
|---|---|---|
| Automatic discovery | Name and description (+`when_to_use`) in context unless disabled; compaction re-attaches invoked skills (5,000 tokens each, 25,000 total). | Name, description, and file location injected into the system prompt as `<available_skills>` XML; the model loads `SKILL.md` with its `read` tool. No dedicated skill tool. |
| Manual invocation | `/skill-name` (or `/plugin:skill`). | `/skill:name [args]`. `enableSkillCommands` (default true) controls menu discovery; typed commands still work if disabled. Body is wrapped in `<skill name=... location=...>` with a "references are relative to <dir>" line; args are appended after the block. |
| Argument substitution | `$ARGUMENTS`, `$0`, `$name`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_SKILL_DIR}`, `${CLAUDE_PROJECT_DIR}`, etc. | None for skills: args are appended as plain text. A body containing `$ARGUMENTS` or `${CLAUDE_SKILL_DIR}` is passed through literally (source: `_expandSkillCommand`). |
| Hiding/overriding | `skillOverrides` setting, `Skill(...)` permission rules. | `skills` setting with `!pattern`, `+path`, `-path`; `--no-skills`, `--skill <path>`. |
| Reload | File watcher; `/reload-skills` for new top-level dirs. | `/reload`. |

## 3. Directory layout and search paths

| Path | Claude Code | pi |
|---|---|---|
| `~/.claude/skills/<n>/SKILL.md` | Yes (personal) | No (no `.claude` path in the pi loader) |
| `~/.agents/skills/` | Not listed in CC-skills | Yes (always treated as user scope) |
| `~/.pi/agent/skills/` | No | Yes (user scope; agent dir) |
| Project `.claude/skills/` | Yes, from start dir up to repo root; nested `<subdir>/.claude/skills` load lazily on first file touch | No |
| Project `.agents/skills/` | Not listed | Yes, from cwd through ancestors, stopping at repo root |
| Project `.pi/skills/` | No | Yes (project scope; needs project trust) |
| Plugins | `<plugin>/skills/<n>/SKILL.md`, namespaced `/plugin:skill` | Pi packages (npm/git/local) with a `skills/` dir or `pi` manifest; not namespaced, collisions keep first (pi-packages) |
| Extra locations | `--add-dir`; enterprise managed dir | `skills` array in settings (user paths relative to agent dir, project paths to `.pi`), `--skill` |
| Legacy | `.claude/commands/*.md` still works; skill wins on conflict | Prompt templates are a separate resource type |

Discovery shape: Claude Code expects `<skills-dir>/<name>/SKILL.md`. pi walks
recursively: a directory that contains `SKILL.md` is a skill and recursion
stops there; otherwise it descends, skipping dot-entries and `node_modules`,
and honors `.gitignore`, `.ignore`, `.fdignore`. In user/project skill roots,
loose `.md` files with a `description` are also accepted as skills (pi-src;
pi-skills says only that "some standalone Markdown skills" are accepted).
Precedence on collisions: Claude Code enterprise > personal > project, with
plugins namespaced; pi keeps the first discovered and warns.

On this machine (observed as data): `~/.claude/skills/*` and
`~/.pi/agent/skills/*` are both relative symlinks to `~/.agents/skills/*`.
`~/.agents/skills/<n>` is read by pi natively and by Claude Code only through
a symlink or copy under `~/.claude/skills/`.

## 4. Supporting files

Both follow the AS-spec convention: `scripts/`, `references/`, `assets/`, any
other files, referenced by relative path, `SKILL.md` under about 500 lines.
Differences:

- Claude Code can give scripts a stable path through `${CLAUDE_SKILL_DIR}`
  (substituted in the body and in `allowed-tools` Bash rules). It does not
  expand in pi.
- pi tells the model the skill's location (`<location>` in the prompt;
  "References are relative to <baseDir>" on `/skill:`), so relative paths work
  if the model resolves them; the prompt tells the model to use absolute paths
  in tool calls.
- Claude Code plugin skills have `${CLAUDE_PLUGIN_ROOT}`/`${CLAUDE_PLUGIN_DATA}`.
- Neither executes scripts on discovery; execution is by the model through a
  shell tool (pi docs: project skills "can instruct the model to run scripts",
  review before trust).

## 5. Shell injection, hooks, and tool vocabulary

- Claude Code: inline `` !`cmd` `` (only at line start or after whitespace) and
  ```` ```! ```` blocks run when the skill renders, output replaces the
  placeholder before the model sees it; failure aborts the invocation; gated
  by permission rules and `allowed-tools`; disabled by
  `disableSkillShellExecution`. Per-skill `hooks` frontmatter exists.
- pi: no pre-render shell step. A `` !`cmd` `` or ```` ```! ```` block would
  reach the model as literal text, so the model would see the command but not
  its output. (Inferred from `_expandSkillCommand`, which only strips
  frontmatter and wraps; not tested at runtime.) Hooks in pi belong to
  TypeScript extensions (`docs/extensions.md`), not to skills.
- Tool names in skill bodies: Claude Code skills commonly mention tools such as
  `Read`, `Grep`, `Bash`, subagents, and `allowed-tools` patterns such as
  `Bash(git status *)`. pi's loader gives no tool-name mapping and its tool
  set differs by session (extensions can add or remove tools; `_allowedTools`
  is session-level). The pi tool inventory beyond `read` and bash was not
  enumerated.

## 6. Subagents and delegation

- Claude Code: `context: fork` plus `agent:` (`Explore`, `Plan`,
  `general-purpose`, or `.claude/agents/*`) and `background`; the reverse is a
  custom subagent with a `skills` field. `disable-model-invocation` skills
  cannot be preloaded into subagents.
- pi: no subagent concept in the Skill format. Core pi ships an example
  extension `subagent/` (`docs/extensions.md`); this machine installs the
  third-party package `pi-subagent-manager` 1.6.1 (npm, repo
  `championswimmer/pi-subagent-manager`) via `settings.json` packages.
  Delegation is therefore an extension/package capability, per machine, not
  something a Skill can request in frontmatter. That package's docs on how it
  interacts with skills were not read.

## 7. Symlinks

- Claude Code: a skill entry in enterprise, personal, or project locations can
  be a symlink to a directory elsewhere; `SKILL.md` is read from the target;
  the skill loads once if several locations point at one target. Plugin skills
  "handle symlinks differently" (details not in the fetched text).
- pi: symlinked `SKILL.md` files and symlinked directories are followed
  (`statSync`); broken links are skipped silently; duplicates by resolved path
  are skipped silently (distinct from name collisions, which warn). Symlinks
  inside a skill pointing at supporting files are not special-cased by the
  loader; they resolve at read time through the model's tools.
- Sandbox implication (from `docs/design-notes.md`): symlinks that point
  outside a mounted Destination break in a sandbox for both harnesses, so
  materialized copies are the safe form. Not tested here.

## 8. Trust

- Claude Code: `allowed-tools` is not gated by workspace trust; managed
  settings can ignore it (`allowManagedPermissionRulesOnly`) or restrict hooks
  (`strictPluginOnlyCustomization`).
- pi: project `.pi/skills` and project `.agents/skills` (cwd or ancestor)
  require project trust; user `~/.agents/skills` is always treated as user
  scope (pi-security, `trust-manager.js`). pi has no built-in sandbox
  (pi-security).

## Unverified or open

- Claude Code facts come from a summarized fetch of the docs page, not raw
  text; re-check edge-case wording before relying on it.
- Findings for pi are for 1.5.0; releases 1.4.0 and 1.5.0 coexist locally. No
  other version checked.
- Runtime behavior of Claude-only constructs under pi (literal pass-through of
  `!` blocks and `$ARGUMENTS`) is inferred from source, not run.
- Whether pi dedupes the same skill reachable through both `~/.agents/skills`
  and `~/.pi/agent/skills` symlinks: source indicates a real-path set, so yes,
  silently; not run.
- pi tool inventory, and how third-party subagent packages use Skills.
- Claude Code plugin-skill symlink behavior. `.agents/skills` is not mentioned
  in the Claude Code docs read, so "not supported" means "not documented".

## What a per-harness Adaptation must touch

| Concern | Claude Code to pi | pi to Claude Code |
|---|---|---|
| Frontmatter | Keep `name`, `description` (must be non-empty), `disable-model-invocation`, `license`, `compatibility`, `metadata`. Report or drop `user-invocable`, `when_to_use`, `allowed-tools`, `disallowed-tools`, `model`, `effort`, `context`, `agent`, `background`, `hooks`, `paths`, `arguments`, `argument-hint`, `shell`. Merge `when_to_use` into `description` (cap 1024). | Mostly unchanged; `name`/`description` portable. |
| Invocation text | Rewrite `/name` mentions to `/skill:name`. | Rewrite `/skill:name` to `/name`. |
| Substitutions | `$ARGUMENTS`, `$N`, `${CLAUDE_SKILL_DIR}`, `${CLAUDE_PROJECT_DIR}`, `${CLAUDE_*}` do not expand: rewrite to relative paths or prose. | None needed. |
| Shell injection | `` !`cmd` `` and ```` ```! ```` do not run: convert to an instruction for the model to run the command, or flag as unsupported. | None needed. |
| Tool names / permissions | Remap or flag tool names in the body; `allowed-tools` has no effect. | Flag assumptions about pi tools. |
| Fork/subagent | `context: fork` and `agent` unsupported in the Skill format: report, or rewrite as an instruction if a delegation extension is present. | Not applicable. |
| Supporting files | Usually unchanged; scripts referenced with `${CLAUDE_SKILL_DIR}` need a path rewrite. | Unchanged. |
| Name | Ensure it satisfies AS-spec and matches the directory name (portable). | Same. |
| Plugin-only | `${CLAUDE_PLUGIN_*}` and namespacing have no pi analogue. | Not applicable. |

## What an install Destination per harness looks like

| Harness | User Destination | Project Destination | Notes |
|---|---|---|---|
| Claude Code | `~/.claude/skills/<name>/SKILL.md` | `<repo>/.claude/skills/<name>/` | Docs do not list `.agents`. Symlinked dirs allowed; materialize for sandbox. Plugin form `<plugin>/skills/` is separate. |
| pi | `~/.pi/agent/skills/<name>/` or shared `~/.agents/skills/<name>/` | `<repo>/.pi/skills/<name>/` or `<repo>/.agents/skills/<name>/` | Project locations need project trust. Extra paths via `skills` setting or package. Does not read `.claude`. |
| Shared neutral tree | `~/.agents/skills/<name>/` (pi native; Claude Code only via link/copy into `~/.claude/skills/`) | `<repo>/.agents/skills/<name>/` (pi native) | Fits the harness-neutral tree idea in `docs/design-notes.md`; content must stay in the portable subset or be adapted per harness. |

Implication: a single Destination cannot serve both harnesses natively. A
shared `.agents` tree plus a Claude Code Destination (link or materialized
copy) covers both, with a per-harness Adaptation only where non-portable
frontmatter or Claude-only constructs are present.
