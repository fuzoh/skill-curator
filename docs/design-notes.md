# Design ideas and open questions

Status: proposals only. No architecture, command names, or file formats have been
selected.

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
- Which existing skill installers overlap with this goal, and where should this
  tool integrate rather than duplicate their work?
- Which source layouts and supporting-file conventions should be supported first?
- Should fetching use Git clones, archives, or an existing installer?
- How should skill identity survive renames, moves, or source removal?
- Which adaptations can be deterministic, and which need model assistance?
- How are conflicts, review approval, rollback, and local edits represented?
- Which target environment should be validated first?
- Should installation copy, link, or export content to another tool?
- What belongs in a portable manifest versus machine-local configuration?
- Which language, distribution mechanism, and license-compatible dependencies fit
  the validated scope?

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
