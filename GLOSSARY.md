# Skill curation

Vocabulary for a local tool that downloads third-party agent skills, adapts
them to the user's tools and harnesses, and keeps them updatable.

## Language

**Source**:
An upstream repository from which skills are fetched.
_Avoid_: Registry, collection, plugin

**Skill**:
A directory holding a `SKILL.md` and its supporting files, selected from one
Source or authored locally.
_Avoid_: Command, playbook

**Revision**:
An immutable commit of a Source that a Skill is pinned to.
_Avoid_: Version, tag, ref

**Adaptation**:
The maintained difference between a Skill at its pinned Revision and the
version the user has accepted.
_Avoid_: Patch, fork, override, customization

**Destination**:
A configured path where accepted Skills are installed.
_Avoid_: Target, output, install dir

**Manifest**:
The user-owned, portable file describing which Skills are selected, from which
Source and Revision, with which Adaptation, to which Destination.
_Avoid_: Config, lock file, registry

**Collection**:
The user-owned, portable repository holding the Manifest, resolved Revisions,
and written Adaptation intents. Lives outside the tool's source tree.
_Avoid_: Dotfiles, config repo, profile

**Composition**:
A Skill produced from several source Skills and a written intent; the third
mode alongside copying a Skill unchanged and adapting one.
_Avoid_: Merge, bundle, meta-skill
