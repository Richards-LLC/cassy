### Changed — Factory

- **A worker's sandbox covers only its own task** — Was: a macOS Claude
  worker's sandbox let Bash write every task's artifact directory in the
  project and the whole staging scratch root. Now:
  - The profile grants the worker's current tasks' artifact directories and its
    checkout's own temporary and build directories, plus the checkout, git paths
    and the operator's write roots as before.
  - Starting a task the running profile does not cover relaunches the worker
    through the normal recycle, which keeps the task started and resumes it.
