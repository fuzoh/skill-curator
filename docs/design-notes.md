# Design ideas and open questions

Status: exploratory proposals except for the decisions linked under **Planning
map**. Command names and file formats have not been selected.

## Possible content lifecycle

1. **Fetch:** retrieve a selected upstream revision into a local source cache.
2. **Select:** identify the skill and its supporting files.
3. **Prepare:** retain the original or produce a proposed adaptation.
4. **Review:** inspect content changes, provenance, and compatibility warnings.
5. **Install:** write accepted content to a configured destination.
6. **Update:** fetch a candidate revision and repeat preparation and review.

Keeping source content, proposed adaptations, and installed output separate could
make changes easier to explain and recover. The storage mechanism remains open.

## Possible adaptation modes

- **Copy:** preserve the selected content unchanged, including supporting files.
- **Adapt:** maintain explicit changes against an identifiable source revision.
- **Author:** maintain user-owned content, with optional references to influences.

These are conceptual categories, not configuration keys. Attribution alone does
not establish permission to reuse source material.

For adapted skills, an update review could compare the previous upstream version,
the accepted local version, and a new upstream version. Whether changes are
represented as patches, transformation rules, written intent, or a combination
needs validation.

Model-assisted rewriting should be optional. Generated changes are proposals,
not authoritative updates. Reproducibility may mean preserving accepted output
and its inputs rather than expecting identical model output on every run.

## Configuration and provenance

A user-owned manifest could record:

- Source repository, selected paths, and immutable revision.
- Skill identity and any local naming choices.
- Adaptation intent and accepted changes.
- Target profile, required tools, and destination.
- License and attribution references.

A separate lock or history mechanism may be useful for resolved revisions and
accepted artifacts. Its relationship with the user's version control is undecided.
Secrets and machine-specific credentials should stay outside shared manifests.

## Environment and sandbox compatibility

Profiles or adapters could express differences between agent environments:
metadata support, tool names, file layout, manual invocation, and delegation.
Prefer explicit capability checks to assuming that rewriting a tool name makes a
skill compatible.

When skills are used by autonomous or orchestrated agents running inside
sandboxes (for example namespace sandboxes or container runtimes):

- **Self-contained installation:** Installing skills as materialized files
  rather than symlinks to external repositories allows a sandbox launcher to
  bind-mount the destination directory read-only without exposing the rest of
  the host filesystem.
- **Runtime capability declarations:** Skills that invoke bundled scripts,
  require specific host binaries, or need network egress could declare those
  requirements in the manifest or profile metadata so a sandbox or orchestrator
  can validate compatibility or filter skills by profile (such as interactive
  versus headless execution).
- **Multi-machine parity:** Separating portable manifests and lock state from
  machine-local destination paths allows keeping an interactive workstation and
  an autonomous server synchronized on the same curated skill revisions.
- **Multi-harness workflows:** Orchestrated pipelines that mix several agent
  harnesses across steps benefit from both shared harness-neutral skill trees
  and harness-specific adaptations generated from the same source definitions.

Composition is another possibility: an entry-point skill can reference smaller
skills or principles instead of combining everything into a single prompt. The
tool's role in managing those dependencies is still an open question.

## Safety considerations

Upstream repositories are untrusted input. Fetching and adapting content should
not execute it. Installation must constrain writes to approved destinations,
handle symlinks and path traversal safely, and detect conflicting local edits.
Model-assisted processing also needs an explicit policy for what content may be
sent to an external provider.

## Decisions to explore

- Who is the first user, and what is the smallest useful update workflow?
- Which source-layout conventions should be reused from existing installers?
  Independence from `npx skills` is settled; see **Technical foundation** below.
- Which source layouts and supporting-file conventions should be supported first?
- How should fetching and caching use the allowed system Git prerequisite?
- How should skill identity survive renames, moves, or source removal?
- Which adaptations can be deterministic, and which need model assistance?
- How are conflicts, review approval, rollback, and local edits represented?
- Which target environment should be validated first?
- Should installation copy, link, or export content to another tool?
- What belongs in a portable manifest versus machine-local configuration?
- Which exact crate versions, features, and dependency-policy settings fit the
  validated scope? Language and initial distribution are settled below.

## Suggested next step

Agree on one concrete user journey and its acceptance criteria. Test that journey
with a small sample before committing to a general configuration schema or
extensibility framework.

## Planning map

Decisions are now tracked on the wayfinder map,
[Map: skill-curator v1 spec and personal Collection](https://github.com/fuzoh/skill-curator/issues/1),
and its child tickets. Resolved terms live in [GLOSSARY.md](../GLOSSARY.md).

Settled while charting the map (2026-11-05):

- The first user is the project owner, on an interactive workstation and a
  headless server, with Claude Code and pi as the two target harnesses.
- This repository holds only the tool. The user's Collection (Manifest, lock,
  Adaptation intents) is a separate user-owned repository; whether the tool
  ships an example starter is undecided.
- Copy, Adapt, and Composition are all in the first version.
- The tool never calls a model API itself. It prepares the context and hands
  the rewrite to the user's agent, then validates and records the accepted
  result.
- Out of scope for the first version: sandbox capability declarations,
  per-environment variants, other harnesses, hosted registry, bundling.

### Technical foundation

Resolved on 2026-11-16 in
[Drive or replace npx skills, and implementation language](https://github.com/fuzoh/skill-curator/issues/7):
independent Rust CLI, defensive coexistence with `npx skills`, system Git,
Cargo/source installation, and a strict agent-oriented development harness
inspired by `ub-mode`. The resolution holds the accepted requirements; the
future implementation-spec breakdown must carry them into a bootstrap issue.
Supporting tooling evidence lives in
[Rust quality tooling: tests, mutation, duplication, and lint](https://github.com/fuzoh/skill-curator/issues/28).
No Rust setup has been implemented.

### Collection file formats

Implemented in
[Manifest, lock, and machine config schemas and Collection loader](https://github.com/fuzoh/skill-curator/issues/30);
the Rust types in `src/collection/` are the reference. Choices made there:

- Names of Sources, Skills, presets, Composition inputs, and Destinations are
  single safe path components: a lowercase letter or digit, then up to 63
  characters from `a-z`, `0-9`, `-`, `_`.
- D1 rules are written `frontmatter = { set, remove, keep_only }` and D2 rules
  `files = { exclude, include }`. A key may not be both set and removed, nor
  set while missing from `keep_only`. Presets hold either or both and are
  referenced by Adapt Skills in order through `presets = [...]`, each preset
  at most once. Presets are global: the issue's "presets per Source or group"
  is met by referencing a preset from each Skill that needs it, not by
  attaching it to a Source. Compositions and Destinations write their rules
  inline.
- A Source may name a `branch` to follow: a plain branch name under the
  `git check-ref-format` rules, never a refspec or revision expression. The
  pinned Revision lives only in `lock.toml`, as a full commit identifier.
- `mode = "copy" | "adapt" | "composition" | "local"` selects the Skill
  shape. A Composition needs at least two inputs naming different upstream
  Skills (Source and `upstream_path`) and an `intent` file under `intents/`.
  `intents/<skill>.md` is the suggested name, not a requirement. Intents and
  Fragments are paths from the Collection root, inside `intents/` and
  `fragments/`, and must be regular files reached without symbolic links; a
  Destination lists each Fragment at most once.
- Each Destination declares `harness`, an optional `skills_dir` (defaulting
  per harness, required for `generic`), `enabled` (default true), optional
  `instructions = { file, fragments }`, and install-time `frontmatter` rules.
  `machine.toml` can change `enabled`, `skills_dir`, and `instruction_file`
  for declared Destinations only. Installation paths are never `/` or `~`
  alone, nor the home directory or one of its parents once expanded, and
  never the Collection root, a path inside it, or one containing it
  (compared with the root both as given and with symbolic links resolved).
  Several
  Destinations may name the same paths in the Manifest; on each machine, the
  Skills directories and instruction files of enabled Destinations must be
  distinct and none may lie inside another.
- `lock.toml` starts with `format = 1` and records per Skill `output_hash`,
  `inputs_hash`, and `provenance` entries (Source, URL, `upstream_path`,
  Revision, license files), one per Composition input. `format` is read
  first, so a lock from a newer release asks for an upgrade instead of
  failing on its schema. Hashes are `sha256:` digests over a length-prefixed
  encoding, independent of TOML formatting and file creation order.
- `inputs_hash` covers the Skill's declaration as written, the contents of
  its presets in order (not their names), and its intent bytes. It does not
  merge rules into their effective result, so renaming a preset keeps the
  hash, while moving a rule between a preset and the Skill changes it.

### Source cache, extraction, and licenses

Implemented in
[System Git Source cache, Revision fetching, and license detection](https://github.com/fuzoh/skill-curator/issues/31);
`src/source.rs` and `src/collection/attribution.rs` are the reference.
Choices made there:

- The cache lives in `$XDG_CACHE_HOME/skill-curator` (absolute values only),
  else `~/.cache/skill-curator`. Each Source URL gets one bare repository,
  `sources/<source>-<16 hex digits of the URL's SHA-256>.git`, so changing a
  URL starts a new cache. The first use clones into a temporary directory
  that is renamed into place once complete.
- Every `git` call runs with `core.hooksPath=/dev/null`, `core.fsmonitor`
  off, `protocol.ext.allow=never`, submodule recursion off, automatic
  garbage collection and maintenance off, replace objects ignored, no
  standard input, no template directory at clone time, and the environment
  variables that redirect Git to another repository (`GIT_DIR`,
  `GIT_INDEX_FILE`, `GIT_CONFIG_PARAMETERS`, ...) removed. The user's own
  Git configuration still applies, so credentials and URL rewrites keep
  working.
- The candidate Revision is the tip of the Source's `branch`, or of the
  remote `HEAD` when none is set, fetched without tags into one tracking
  reference, so changing `branch` (even from `release` to `release/v2`)
  never collides with an older tracking reference. Extracting a Revision
  that is not cached fetches it by identifier. When the Source refuses (as
  every protocol v0 or v1 server does for a commit that is not a branch
  tip), every branch is fetched, with pruning, into private mirror
  references; if no branch brings the Revision, the typed
  `RevisionUnreachable` error lets callers fall back to a two-way
  comparison. A Source that cannot be reached
  (network, authentication) stays a plain `git fetch` failure, recognised
  from Git's untranslated (`LC_ALL=C`) diagnostics, so a transient outage is
  never mistaken for a history rewrite. Each extracted Revision is kept by a
  private reference, and automatic garbage collection is off, so a Revision
  fetched once survives a later upstream history rewrite.
- Extraction never checks out a working tree, which also keeps Git
  attributes and filters out of play: it lists the tree at the Revision
  with `git ls-tree` and writes each blob with `git cat-file`. Every
  symbolic link in the Skill directory is refused, not only those pointing
  outside it, because materialised Skills hold regular files only; the
  error says whether each link escapes. Submodules, unusual file modes,
  non-UTF-8 paths, and paths with a `..`, `.`, empty, or `.git` (any letter
  case) component are refused too, all listed in one error. Files land in a
  fresh temporary directory, read-only (`0444`, or `0555` for executables),
  removed when dropped.
- `upstream_path` is a Skill only when it is a directory holding a
  `SKILL.md`. Otherwise the error gives the reason and up to five
  directories holding a regular `SKILL.md`, ranked by matching final name,
  edit distance of that name, shared leading directories, and edit distance
  of the whole path. Nothing is chosen automatically.
- License files are regular files named `LICENSE`, `LICENCE`, `COPYING`, or
  `UNLICENSE` (any case, optionally followed by `.`, `-`, or `_` and more
  text) at the Source root, in each directory leading to the Skill (such as
  `pstack/LICENSE` for `pstack/critique`), and in the Skill directory. The
  summary is the file's `SPDX-License-Identifier` tag, else the GNU license
  named first near the top of the text with the version right after it
  (full GNU texts and the MPL-2.0 name other GNU licenses further down),
  else the Apache-2.0 or MPL-2.0 title near the top, else the first match
  among characteristic phrases of other common licenses, with exclusions
  so that a text is never read as a laxer relative (MIT as MIT-0,
  BSD-3-Clause as BSD-2-Clause), else absent. A text with restricted terms
  (Business Source License, Commons Clause, "Good, not Evil", Server Side
  Public License) is never summarised as the license it names or quotes. A
  near miss stays unidentified rather than getting a wrong summary.
  Regression tests run on full canonical texts. The warning fires when no
  file is found or any found file is unidentified.
- `ATTRIBUTION.md` is rendered from `lock.toml` alone: Skills by name,
  provenance entries by Composition input, Source, and path, every value in
  a code span that no lock value can break out of. It lists, per upstream
  Skill or Composition input, the path, Source, URL, Revision, and license
  files with their summary, or says that no license was found. It is written
  atomically, replacing (never following) a symbolic link at its path.
  License texts are not copied into it.

### Deterministic Adaptation and `materialize`

Implemented in
[Deterministic Adaptation pipeline (D2 filtering, D1 frontmatter) and materialize command](https://github.com/fuzoh/skill-curator/issues/32);
`src/adaptation.rs`, `src/adaptation/`, and `src/materialize.rs` are the
reference. Choices made there:

- D2 rule sets apply in order: presets as listed, then the Skill's own rules
  (a Composition input has its own rules only). A file is kept when every
  rule set keeps it: its `include` globs, when present, select it and none
  of its `exclude` globs do. A glob selects a file when it matches the
  file's path or one of its parent directories, so `evals`, `evals/`, and
  `evals/**` all drop `evals/`; a trailing `/` matches directories only.
  `*` and `?` stay within one path component, `**` crosses any number, `\`
  escapes, matching is case-sensitive. Invalid globs, and globs with empty
  or `.` segments (`./evals`, `evals//x`, which would match nothing), are
  Manifest problems.
- Rules that drop the top-level `SKILL.md` are an error naming the rule,
  never an implicit exception that keeps it.
- D1 edits the frontmatter entry by entry instead of re-serialising it:
  untouched entries keep their exact text, comments, and quoting, and the
  body after the closing `---` is kept byte for byte (fenced blocks,
  horizontal rules, a byte order mark, and CRLF line endings included).
  Rules apply as `remove`, then `keep_only`, then `set`. A set value is
  written as one line of flow YAML: plain when it cannot be read as
  anything else, else double-quoted; TOML dates are written as strings.
  Every edit is parsed again and refused unless it yields exactly the
  expected entries, so an edit that would change an untouched entry (for
  example through a shared anchor) is refused. Frontmatter with several
  documents, duplicate or non-scalar top-level keys, a top-level YAML merge
  key (`<<`, or a key tagged `!!merge`, which merge-aware readers expand into
  top-level keys a `remove` rule could not see), or nesting deeper than
  64 levels is refused too. Aliases are compared by the node they resolve
  to (redefined anchors included), and frontmatter that would exceed
  100,000 nodes once its aliases are resolved is refused. Lines end at
  `\r\n`, `\r`, or `\n`, as YAML line breaks do. Without D1 rules the
  frontmatter is not parsed at all, so a Copy Skill, or an Adapt Skill
  with D2 rules only, keeps a frontmatter D1 could not edit. The same
  helper serves `det(R)`, the re-enforcement of D1 on agent output, and
  install-time Destination rules.
- `det(R)` applies D2, then D1, to an upstream Skill read as regular files
  only. Written Skills are new directories with `0644` or `0755` files.
- Each `lock.toml` entry of an upstream Skill records an `applied` table:
  the D1 and D2 rules (presets expanded, in order), the D2 rules of each
  Composition input, and the hash of the written intent the accepted Skill
  was produced from. `inputs_hash` alone cannot tell a D1 change from a D2
  or intent change, and `materialize` must not need the old Manifest.
  `update` must record it when it accepts a Skill.
- `materialize` never fetches: it reads the pinned Revision from the Source
  cache only and reports a Revision the cache lacks as an error asking for
  `skill-curator update`. It works on the Collection given by
  `--collection`, else the working directory.
- Output drift (files that no longer hash to `output_hash`, a missing
  directory, or a symbolic link or special file inside it) is reported as
  manual edits, and that Skill is left alone. `--accept <skill>` records
  the current files as accepted (reminding the user to update the written
  intent, and warning when `SKILL.md` breaks the Skill's D1 rules or has a
  frontmatter they cannot be applied to), then replays any pending rule
  change on top. `--reset <skill>` rebuilds `det(R)` at the pinned Revision
  with the current rules, discarding the edits; on an agent-assisted Skill
  it records the intent as not applied, so later runs ask for an agent
  rewrite. A reset of a Skill whose Source or `upstream_path` changed is
  refused as needing `update`: the pinned Revision holds the old upstream
  Skill. A Composition has no deterministic state and cannot be reset; its
  Git history can restore it, so the hints printed for a Composition never
  suggest `--reset` (manual edits: `--accept` or the Git history; a missing
  `applied` table: `update`). A local Skill takes neither option: its files
  are recorded on every run.
- Input drift is classified against `applied`. Files newly kept by D2 on
  an agent-assisted Skill need an agent rewrite, even when the Skill still
  equals `det(R)`: the agent never saw them. Apart from that, a Skill still
  equal to `det(R)` under its old rules is rebuilt under the new ones. On
  other Skills (agent rewrites, accepted edits), D2 exclusions remove files,
  D1 changes
  go through a replay that changes only the keys whose upstream value the
  change affects (a key a removed rule used to drop comes back with its
  upstream value) and then enforces the new rules, and files newly kept by
  D2 are copied from the pinned Revision on deterministic Skills. Newly
  kept files and Composition input rule changes on agent-assisted Skills
  (Adapt with an intent, Compositions), and any intent change, are reported
  as needing an agent rewrite or a reset. A removed intent, and a newly
  kept file clashing with an existing one, need a reset. A changed Source
  or `upstream_path` needs `update`.
- Each changed Skill is written to a staging directory inside `skills/`
  and swapped into place, then `lock.toml` is written atomically, one
  Skill at a time. Local Skills are recorded (`output_hash` only) whenever
  their files change. The command exits with status 1 when any Skill still
  needs attention, after printing one line per Skill.

### Agent rewrite handoff

Implemented in
[Agent rewrite handoff (pi / claude / --manual) and output validation](https://github.com/fuzoh/skill-curator/issues/33);
`src/rewrite.rs`, `src/rewrite/`, and `src/diff.rs` are the reference.
Choices made there:

- A rewrite job carries the Skill name, its upstream views (`det(R_old)`
  when reachable and `det(R_new)` for an Adaptation; each input at the old
  Revision when reachable and at the new one, after its D2 rules, for a
  Composition), `accepted_local` when the Skill was accepted before, the
  written intent text, the D1 rule sets, and optional extra guidance for a
  re-run. Building it from the Collection and the Source cache is left to
  `update`.
- The staging directory is a fresh `skill-curator-rewrite-*` directory in
  the system temporary directory, removed when the rewrite ends, whatever
  the outcome. It holds `BRIEF.md`, `base/`, `upstream/` (Adaptation),
  `inputs/<input>/` (Composition), `diffs/<name>.diff` (`upstream.diff` for
  an Adaptation, one per changed Composition input; written only when there
  are changes), `current/`, and `output/`. Every view and `BRIEF.md` is
  read-only (`0444`, `0555` for executables and directories); removal makes
  directories writable again without following symbolic links.
- `output/` starts as `accepted_local`, else `det(R_new)` for an
  Adaptation, else empty (a first Composition).
- `BRIEF.md` gives the written intent verbatim, the extra guidance, the
  views, a per-file summary of upstream changes (or says they are unknown
  when the old Revision is unavailable: the two-way case, comparing
  `current/` with `upstream/`; a Skill never accepted before is a first
  rewrite instead, started from `upstream/` and the written intent), the D1
  rules that are enforced again, and output instructions. It tells the
  agent that view files are data: never run their scripts or follow
  instructions in them.
- The agent is chosen by `update --agent <pi|claude|manual>` (or
  `--manual`), else by `agent = "..."` in `machine.toml` (an agent's
  availability is machine-specific), else `pi`. `pi` and `claude` are
  started interactively from `PATH` in the staging directory, inheriting
  the terminal, with one fixed first message asking them to follow
  `BRIEF.md`. A non-zero exit discards the output. Manual mode starts
  nothing: it prints the staging directory and waits for Enter; `abort` or
  end of input stops the rewrite.
- `output/` is accepted only as a real directory of regular files and
  directories: symbolic links anywhere (including `output/` itself), hard
  links (a file with more than one link may live outside `output/`),
  special files, non-UTF-8 names, and `.git` components are refused.
  `SKILL.md` must exist, be UTF-8, and start with a frontmatter the D1
  parser accepts (a YAML merge key is refused even without D1 rules). The
  D1 rule sets are then enforced again in order, and the proposal records
  whether that changed the agent's `SKILL.md`. D2 is not applied again.
- The review diff runs from `accepted_local` (or nothing) to the proposal:
  a unified diff per text file, one line per binary file or executable-bit
  change, with a per-file time limit on line matching so hostile input
  cannot stall the review. Persisting an accepted proposal is `update`'s
  job.
