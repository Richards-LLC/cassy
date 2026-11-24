## Core Concepts

**Repeated verification rejections (G-170).** After two rejected verdicts, Cassy holds further repairs and reassignments. An independent reviewer publishes a Markdown artifact on the task with an Actor, Operation, Design, and Observed matrix, followed by a Mismatches section listing every discrepancy. A live supervisor links the review with the decision note `whole-flow review: <artifact-id>`. Cassy verifies the published bytes and includes the review in the next repair brief. A later rejection requires a fresh review.

At the same threshold, Cassy detaches the task from its release train, retargets it to the integration branch, and notifies the supervisor. To keep it on the train, record `keep release train: <train-id>: <reason>` as a supervisor decision before the threshold is reached. Keeping the task on the train does not waive review.
