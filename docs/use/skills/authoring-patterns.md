# Skill Authoring Patterns

A good skill is specific, repeatable, and easy to decide whether to use. It should teach a procedure, not describe a vague preference.

## Template

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

## Good Example

```markdown
# Release readiness check

Use this when asked to verify whether a local repository is ready for release.

Inputs:
- Repository path or project name.
- Target release branch, if provided.

Steps:
1. Check the current branch and working tree status.
2. Compare local tags with upstream tags.
3. Inspect recent commits since the last tag.
4. Report blockers, unknowns, and the exact commands used.

Do not:
- Push, tag, or merge without explicit approval.
```

## Weak Example

```markdown
# Be better at releases

Always check everything carefully and do not make mistakes.
```

This is weak because it has no trigger, no concrete steps, no expected inputs, and no tool or reporting guidance.

## Writing Rules

- Use one skill for one repeatable workflow.
- Put the trigger near the top.
- Prefer concrete steps over motivational advice.
- Mention expected tools when they matter.
- Keep the body short enough to load on demand.
- Avoid secrets and private data.
- Include "Do not" rules when the workflow has dangerous actions.

