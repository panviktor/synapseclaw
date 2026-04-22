# Skill Health And Auto-Promotion

Skill health summarizes utility without reading full skill bodies. It uses typed traces and compact records to count selected, read, helped, failed, repaired, blocked, and rollback signals.

```bash
synapseclaw skills health --limit 20 --trace-limit 100
synapseclaw skills health --apply
```

`--apply` performs eligible cleanup lifecycle changes, such as status updates for learned skills. It should not rewrite manual or imported skills just because their metadata needs improvement.

Health is also an input to future ranking and consolidation. A skill that is selected, read, and helpful should become easier to trust; a skill that fails, blocks, or rolls back repeatedly should be reviewed or deprecated.

## Auto-Promotion

```bash
synapseclaw skills autopromote
synapseclaw skills autopromote --apply
```

The dry-run command evaluates generated patch candidates against policy, eval, provenance, target version, and recent live trace signals. The apply command writes only when `[skills.auto_promotion].enabled=true` and the candidate is eligible.

Keep dry-run output as the normal operator view. Treat `--apply` as a controlled write path, not a background habit.
