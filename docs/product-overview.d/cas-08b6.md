## Core Concepts

- **Machine-wide worker census** - spawn admission and `worker_status` read one census of live workers across every local factory project. Both print it as "N/M live on this machine: <project> <count>, …", so a refusal shows which projects fill the limit.
