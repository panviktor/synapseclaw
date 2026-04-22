# Skills Troubleshooting

## Skill Is Not Found

Run:

```bash
synapseclaw skills authored
synapseclaw skills learned
```

If the skill is package-backed, confirm that package porting or imported package indexing is enabled and that the package passed audit.

## Skill Exists But Is Not Used

Check whether the skill is `active`. Candidate and deprecated skills should not normally be loaded by `skill_read`.

## Candidate Cannot Be Applied

Run:

```bash
synapseclaw skills diff <candidate-id>
synapseclaw skills test <candidate-id>
```

Common causes are stale target version, missing replay/eval evidence, failed procedure claims, or policy rejection.

## Rollback Fails

Rollback is version-aware. It should refuse to restore an old snapshot if the target skill has changed since the apply record was created.

## Package Fails Audit

Remove secrets, prompt-injection instructions, credential exfiltration claims, and unsafe tool claims. A package must be safe before it enters the runtime catalog.

## Full Skill Body Is Not In The Prompt

That is expected. The runtime uses compact catalog entries first and loads full instructions through `skill_read` only when needed.

## Embedding Search Misses A Skill

Use exact listing commands to confirm the skill exists. Embedding search is useful for semantic retrieval, but exact id/name lookup and list views remain important debugging tools.

