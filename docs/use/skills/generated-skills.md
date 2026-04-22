# Generated Skills

Synapseclaw can propose skills from repeated useful patterns. Generated material should be treated as a candidate until it passes review, replay/eval gates, and policy checks.

## Generated New Skills

A generated new skill is a proposed procedure that does not yet replace an existing skill. It should remain a candidate until an operator reviews whether it is specific, safe, and useful enough to become active.

Use generated new skills for recurring workflows that are not already covered by an active skill. Do not use them for one-off facts, generic dialogue, or vague preferences.

## Generated Patch Candidates

A generated patch candidate improves an existing skill. The patch must match the current target version, carry enough provenance, and pass replay/eval checks before it can be applied.

Patch candidates are safer than free-form rewrites because they target one existing skill and one known version. A stale patch should be rejected instead of silently overwriting a newer version.

## Auto-Promotion

Auto-promotion is policy-gated. It should apply only eligible generated patches when `[skills.auto_promotion].enabled=true` and the candidate passes evaluation, provenance, version, and live trace checks.

Auto-promotion is appropriate for narrow, repeatable improvements with strong evidence. It is not a substitute for operator review of broad behavior changes or new trust boundaries.
