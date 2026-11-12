# Skill format and frontmatter requirements for Codex and Antigravity (`agy`)

Status: research findings, 2026-11-12. Answers issue #19, extending [docs/research/harness-skill-formats.md](https://github.com/fuzoh/skill-curator/blob/research/harness-skill-formats/docs/research/harness-skill-formats.md) (issue #4). Checked against local primary documentation on this machine:

- **OpenAI Codex CLI**: official Codex manual (`https://developers.openai.com/codex/codex-manual.md`, cached via `~/.codex/skills/.system/openai-docs/scripts/fetch-codex-manual.mjs`, lines 27280–27600 and 33828–33980) and bundled system skills (`~/.codex/skills/.system/skill-creator/SKILL.md`, `references/openai_yaml.md`, `scripts/quick_validate.py`).
- **Google Antigravity (`agy`)**: built-in customization guides (`/home/bastien/.gemini/antigravity-cli/builtin/skills/agy-customizations/SKILL.md` and `docs/{skills,rules,plugins,json_configs}.md`, plus `/home/bastien/.gemini/antigravity-cli/builtin/skills/antigravity_guide/SKILL.md` and `references/cli.md`).

---

## 1. Discovery paths (Skills and global instruction files)

| Scope | Claude Code (from #4) | `pi` (from #4) | OpenAI Codex | Antigravity (`agy`) |
| --- | --- | --- | --- | --- |
| **User / global skills** | `~/.claude/skills/<n>/SKILL.md` | `~/.pi/agent/skills/`, `~/.agents/skills/` | `~/.agents/skills/<n>/SKILL.md` (documented primary user scope; also `$CODEX_HOME/skills` / `~/.codex/skills` for system/installed skills) | `~/.gemini/antigravity-cli/skills/<n>/SKILL.md` (and `~/.gemini/config/skills.json` declarations) |
| **Repo / project skills** | `.claude/skills/<n>/SKILL.md` | `.pi/skills/`, `.agents/skills/` | `.agents/skills/<n>/SKILL.md` (walked from `$CWD` up to `$REPO_ROOT`) | `.agents/skills/<n>/SKILL.md` (walked from `$CWD` up to repo root; also `.agent/`, `_agents/`, `_agent/`, or `.agents/skills.json`) |
| **Symlink support** | Follows symlinks | Follows symlinks | Follows symlinks for local discovery (`~/.agents/skills`), **but plugin packager rejects symlinks** (`skill_symlink_ignored`: "Symbolic links directly under `skills/` aren't imported as skills; each skill must be a real directory") | Follows symlinks locally (current `~/.gemini/antigravity-cli/skills/*` are symlinks to `~/.agents/skills/*`) |
| **Global instruction file** | `~/.claude/CLAUDE.md` | `~/.pi/agent/AGENTS.md` | `~/.codex/AGENTS.md` (and hierarchical `AGENTS.md` in repos) | `~/.gemini/config/` rules and hierarchical `GEMINI.md` / `AGENTS.md` (24 KB per-file cap, 20,000-token total budget) |

### Key takeaway for Destinations (#13)
- **Project level**: `.agents/skills/<name>/SKILL.md` is shared by `pi`, Codex, and `agy` (only Claude Code requires `.claude/skills/`).
- **User level**: `~/.agents/skills/<name>/SKILL.md` is natively read by both **`pi` and Codex**! Claude Code needs `~/.claude/skills/` and `agy` uses `~/.gemini/antigravity-cli/skills/` (or a `skills.json` pointer in `~/.gemini/config/skills.json` pointing `entries` at `~/.agents/skills` or the Collection).
- **Materialised copies**: Codex's plugin validator explicitly ignores symlinked skill directories (`skill_symlink_ignored`), reinforcing the choice to materialise real directories on install.

---

## 2. YAML Frontmatter & Invocation Policy

| Feature / Field | Claude Code | `pi` | OpenAI Codex | Antigravity (`agy`) |
| --- | --- | --- | --- | --- |
| **Required fields** | `name`, `description` (falls back to body) | `name`, `description` (non-empty required) | `name` (<=64 chars, `[a-z0-9-]+`), `description` (<=1024 chars, non-empty) | `name` (lowercase hyphenated), `description` (required) |
| **Strict validator / rejected fields** | Local loader ignores unknown keys; **Skills API / claude.ai upload** allows only `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools` | Loader picks only `name`, `description`, `disable-model-invocation` and ignores others | `quick_validate.py` **rejects** any frontmatter key outside `{"name", "description", "license", "allowed-tools", "metadata"}` and **rejects `<` or `>` in `description`** | Docs specify `name` and `description`; runtime loads skills from `~/.agents/skills` containing `disable-model-invocation`, `argument-hint`, and `metadata` without error |
| **Manual-only (disable auto-discovery)** | `disable-model-invocation: true` in `SKILL.md` frontmatter | `disable-model-invocation: true` in `SKILL.md` frontmatter | **`agents/openai.yaml`** inside the skill folder with:<br>`policy:`<br>`  allow_implicit_invocation: false`<br>(or per-user `[[skills.config]]` in `~/.codex/config.toml`) | All discovered skills in `~/.gemini/antigravity-cli/skills` are listed in `<skills>`; slash commands are invoked as `/name` |
| **Manual invocation syntax** | `/skill-name` | `/skill:skill-name` | `$skill-name` (or `/skills` menu) | `/skill-name` (or model reads `SKILL.md` via `read_file`) |

### Crucial differences for deterministic install-time transforms (#5, #8, #13)
1. **Manual-only invocation (`disable-model-invocation: true` vs `agents/openai.yaml`)**:
   - Claude Code and `pi` read `disable-model-invocation: true` in `SKILL.md` frontmatter.
   - **Codex does NOT use `disable-model-invocation: true`**. Instead, Codex reads `<skill-dir>/agents/openai.yaml`:
     ```yaml
     policy:
       allow_implicit_invocation: false
     ```
   - Furthermore, Codex's `quick_validate.py` rejects `disable-model-invocation` (and `argument-hint`) if present in `SKILL.md` frontmatter (`allowed_properties = {"name", "description", "license", "allowed-tools", "metadata"}`), and Codex caps `skills.max_context_tokens` (default 2% of context window / 8,000 chars), truncating descriptions or dropping auto-discovered skills with a warning when too many skills are implicitly enabled!
   - **Deterministic solution for `skill-curator`**:
     - When a Skill has `disable-model-invocation: true` (e.g., Tier 2 `principle-*` leaves or manual-only skills), `skill-curator` can deterministically generate `<skill-dir>/agents/openai.yaml` with `policy: { allow_implicit_invocation: false }` (or strip non-standard frontmatter keys if installing to a strict Codex Destination).
2. **`description` constraints in Codex**:
   - Must be $\le 1024$ characters and must not contain angle brackets (`<` or `>`) per `quick_validate.py`.

---

## 3. Command expansion (```` ```! ````) and variable substitutions

- **Neither Codex nor `agy` expands Claude Code's ```` ```! ```` / `` !`cmd` `` shell blocks or `$ARGUMENTS` / `${CLAUDE_SKILL_DIR}` pre-prompt.**
- Like `pi`, both Codex and `agy` load `SKILL.md` by reading the file (`read_file` in `agy`, file load in Codex) when the skill is activated.
- Consequently:
  - Any ```` ```! ```` fenced command block in `SKILL.md` is read by the model as a normal Markdown code block containing a shell command, which the model can execute via its bash/run_command tool. It is **not blocking**.
  - `${CLAUDE_SKILL_DIR}` is not expanded by `pi`, Codex, or `agy`; skills should reference bundled scripts/references using paths relative to the skill directory (which all four harnesses resolve from the known `SKILL.md` path).
