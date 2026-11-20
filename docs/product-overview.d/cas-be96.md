## Core Concepts

- **Delivery CI watches stay within the GitHub budget.** A parked delivery gets
  a CI-completion watch only when its project defines the workflow being
  watched and the delivery is not already on its target branch. The landing
  watch still reports when the delivery merges. Watches for a workflow the
  project does not define are never polled.
