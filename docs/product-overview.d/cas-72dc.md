## Core Concepts

Each MCP backend records its PID and version when its daemon starts. That record is the only proof of the live server's version for `worker_status`, the post-install stale-server notice and Commander's Reload Cassy. The write takes its database lock up front, so another factory process writing at the same moment, for example during a reconnect or a launch burst, delays the record instead of discarding it.
