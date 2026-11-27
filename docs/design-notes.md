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
