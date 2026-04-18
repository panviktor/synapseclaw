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

