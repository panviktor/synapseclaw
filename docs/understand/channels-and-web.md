# Channels And Web

Web and channel entry points should share parser, executor, formatter, and lifecycle behavior wherever the user-visible command is the same. They should differ only where transport, authentication, delivery, or side effects require it.

The `/skills` command path is the reference example. It uses shared behavior so that web and channel flows do not drift apart.

