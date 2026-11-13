## Core Concepts

Factory and epic pushes require local fast checks. A push changing `hub-web/` also proves that committed dist matches a fresh build. Supervisor startup verifies the main checkout's configured Git hook directory using the same read-only check as doctor, which names the tracked `.githooks` repair when available.
