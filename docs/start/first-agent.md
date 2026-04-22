# First Agent

An agent is a configured runtime identity with model settings, tools, memory, and optional channel presence. Helper agents use the same core runtime ideas as the main service, but with separate configs and service names.

The important rule is that behavior should stay shared where possible. Web and channel entry points should differ in transport and lifecycle, not in duplicated command logic.

