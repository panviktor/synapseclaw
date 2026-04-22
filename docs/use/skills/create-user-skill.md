# Create a User Skill

User-authored skills are the normal way to teach Synapseclaw a repeated workflow. You write ordinary Markdown; the runtime adds governance metadata, audit, status, provenance, versioning, and retrieval metadata.

You do not need to write internal contracts manually. Use clear instructions, optional task/tool metadata, and avoid secrets.

## CLI

```bash
synapseclaw skills create \
  --name "Release checklist" \
  --task-family "release" \
  --tools git_operations,file_read \
  --tags release,git \
  --body "# Release checklist

1. Inspect the current branch and tags.
2. Check whether the working tree is clean.
3. Summarize release risks before changing anything."
```

## Runtime Command

```text
/skills create Release checklist --task-family=release --tools=git_operations,file_read --tags=release,git :: Inspect branch, tags, clean state, and release risks before changing anything.
```

## Web UI

Open the Skills page and use the Authoring panel. Provide the name, description or body, task family, tool hints, and tags, then create the skill.

## Good Skill Shape

A good skill covers one repeatable task. It should have a clear trigger, concrete steps, expected tools if known, and a short body.

Do not store tokens, passwords, OAuth callbacks, device codes, or private customer data in a skill. A skill should describe how to do work, not contain secrets needed to do it.

## Recommended Template

```markdown
# Skill name

Use this when...

Inputs:
- ...

Steps:
1. ...
2. ...
3. ...

Report:
- ...

Do not:
- ...
```

## Strong Example

```markdown
# Matrix release check

Use this when asked whether a local Matrix server checkout is behind upstream.

Inputs:
- Local repository path, if provided.
- Upstream repository URL, if not already known.

Steps:
1. Find the local Matrix repository.
2. Read the current local tag or release version.
3. Read upstream release tags.
4. Compare semantic versions.
5. Report whether the local checkout is current, outdated, or unknown.

Do not:
- Pull, checkout, or restart services unless the user explicitly asks.
```

## Weak Example

```markdown
# Be careful with Matrix

Always inspect the repository and make a good decision.
```

This is weak because it has no trigger, no concrete steps, no expected output, and no safety boundary. Use [authoring-patterns.md](authoring-patterns.md) for more examples.
