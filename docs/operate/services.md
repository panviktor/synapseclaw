# Services

The local fleet usually includes the main `synapseclaw.service` and helper services such as `copywriter`, `marketing-lead`, `news-reader`, `publisher`, and `trend-aggregator`. Each service should report `active` after a successful deploy.

Helper agents use the same runtime primitives as the main service, including shared skills behavior. If one service is active but not healthy, inspect its user journal before changing code.

