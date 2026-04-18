# Skills Quickstart

This flow creates a user-authored skill, makes it discoverable, and checks that the runtime can use the governed activation path. It is a good first workflow because skills are the most mature user-facing subsystem.

## 1. Create a Skill

```bash
synapseclaw skills create \
  --name "Matrix release check" \
  --task-family "release-audit" \
  --tools repo_discovery,git_operations \
  --tags matrix,release \
  --body "# Matrix release check

Find the local Matrix repo, read local version tags, compare with upstream tags, and report whether the install is outdated."
```

You can also create the same kind of skill from a runtime command:

```text
/skills create Matrix release check --task-family=release-audit --tools=repo_discovery,git_operations --tags=matrix,release :: Find local Matrix repo, compare tags, report outdated status.
```

## 2. List Authored Skills

```bash
synapseclaw skills authored
```

The new skill should appear as a memory-backed manual skill. If it does not, check that the daemon is reachable or rerun the command with direct memory access according to your local setup.

## 3. Use the Skill

Ask the agent for a task that matches the skill:

```text
Check whether the local Matrix server checkout is behind the current upstream release.
```

The model should see a compact skill catalog entry when the skill is relevant. It should load the full body only through `skill_read`, not by receiving every skill body in the initial prompt.

## 4. Inspect Health

```bash
synapseclaw skills health --limit 20 --trace-limit 100
```

Health output shows compact utility counters such as selected, read, helped, failed, repaired, blocked, and rollbacks. These signals help decide whether a skill is useful, stale, or needs review.

