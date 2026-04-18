# Understand Synapseclaw

This section explains the runtime architecture for readers who need more than the user guide. It is not the first-run path; use it when you need to reason about memory, skills, channels, replay, or extension boundaries.

The core theme is compact, governed runtime behavior. Shared decisions should live in common services and ports, while adapters handle transport and concrete side effects.

