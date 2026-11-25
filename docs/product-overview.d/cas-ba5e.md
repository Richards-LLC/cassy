## Core Concepts

**Repeated verification rejections (G-172).** After two rejected verdicts across the task's lifetime, Cassy holds further repairs, reassignments, and new starts. An independent reviewer publishes a Markdown artifact on the task with an Actor, Operation, Design, and Observed matrix, followed by a Mismatches section listing every discrepancy. A live supervisor links the review with the decision note `whole-flow review: <artifact-id>`. Cassy verifies the published bytes and includes the review in the next repair brief. A later rejection requires a fresh review.

At the same threshold, Cassy detaches the task from its release train, retargets it to the integration branch without discarding its recorded source, and notifies the supervisor. The daemon recovers a missed notice. To keep it on the train, record `keep release train: <train-id>: <reason>` as a supervisor decision before the threshold is reached. Keeping the task on the train does not waive review.
