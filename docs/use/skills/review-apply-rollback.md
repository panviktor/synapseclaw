# Review, Apply, And Roll Back Skills

Generated patches should go through review before they modify an active skill. The normal operator flow is diff, test, apply, inspect versions, and roll back if needed.

```bash
synapseclaw skills candidates
synapseclaw skills diff <candidate-id>
synapseclaw skills test <candidate-id>
synapseclaw skills apply <candidate-id>
synapseclaw skills versions <skill-id-or-name>
synapseclaw skills rollback <apply-record-or-ref>
```

## Apply Rules

Apply requires a matching target skill version. Stale patches should be rejected instead of silently rewriting a newer skill.

Replay/eval gates must pass before a generated patch is applied. The previous version is stored as a rollback snapshot outside normal active retrieval, and rollback preserves audit history.

## API Equivalents

- `POST /api/skills/candidates/diff`
- `POST /api/skills/candidates/test`
- `POST /api/skills/candidates/apply`
- `GET /api/skills/versions`
- `POST /api/skills/rollback`

Use the web Skills page for the same flow when a browser-based operator workflow is easier.

