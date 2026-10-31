# Goals and candidate features

Status: exploratory. These are product goals and feature candidates, not a
committed specification or implementation plan.

## Problem

Skills are often collected from several repositories. Some work unchanged; others
need adjustments for different tools, agent environments, or workflows. Updating
them becomes difficult once local adaptations diverge from upstream.

The tool should make those relationships explicit and keep the user in control
of what is downloaded, changed, and installed.

## Scope

Build a configurable local tool, not a bundled skill collection or hosted skill
registry. It downloads selected third-party content directly onto the user's
machine and helps maintain that content over time.

User manifests, personal skills, generated output, and credentials belong outside
this project's source tree. Users should be able to version their own collection
independently of the tool.

## Candidate capabilities

### Sources and selection

- Register and remove upstream repositories.
- Select individual skills rather than installing every skill from a source.
- Pin immutable revisions and record source paths and attribution.
- Surface available license information and flag missing or unclear licenses.
- Include user-authored skills alongside imported ones.

### Adaptation

- Preserve selected skills unchanged when customization is unnecessary.
- Maintain targeted changes to tool names, instructions, or agent assumptions.
- Support optional model-assisted rewrites with explicit user review.
- Describe adaptation intent so later updates can preserve it.
- Reset an adaptation to its source version or revise its adaptation rules.
- Support independently authored skills informed by cited references.

### Updates and history

- Compare pinned source versions with candidate upstream revisions.
- Distinguish upstream changes, local changes, and proposed merged output.
- Review updates and conflicts before replacing installed content.
- Track accepted source revisions and adaptation history.
- Restore previously accepted versions.

### Profiles and installation

- Configure destination paths instead of assuming a particular machine layout.
- Select variants based on the target environment and available tools.
- Support manually invoked skills, discoverable skills, and composed workflows
  where the target environment supports them.
- Report unsupported capabilities rather than silently changing their meaning.
- Preview installation changes and protect unrelated files and local edits.

## Boundaries

- No bundled or redistributed third-party skill collection.
- No silent acceptance of model-generated adaptations or upstream updates.
- No assumption that every agent supports identical skill metadata, invocation
  behavior, or subagent features.
- No automatic execution of downloaded instructions or scripts during fetching,
  inspection, or update review.

Downloading rather than bundling does not remove upstream license obligations.
Source attribution and applicable notices must remain available locally.

## Initial validation

Before building a broad framework, validate a small end-to-end workflow: select a
skill, pin its source, keep or adapt it, install it to an explicit destination,
then review an upstream change. Use that experiment to choose the first release
scope.
