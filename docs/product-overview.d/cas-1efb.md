## Core Concepts

Factory and supervisor-authored epic pushes require local fast checks, including changed release-note drafts. A push changing `hub-web/` also proves committed dist matches a fresh build. The integration engine reuses already checked delivery content with a marker bound to the exact pushed head and observed remote base; a later supervisor commit cannot reuse that exemption. Supervisor startup and doctor verify the main checkout's configured hook directory and name its repair.
