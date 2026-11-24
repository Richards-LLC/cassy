## Core Concepts

**Repeated verification rejections (G-170).** After two rejected verdicts, Cassy holds further repairs and reassignments. An independent reviewer compares each actor and operation against the design and publishes the table of all mismatches on the task. A live supervisor links that artifact with the decision note `whole-flow review: <artifact-id>`. The next repair brief carries that reference and covers all mismatches. A later rejection requires a fresh review.

At the same threshold, Cassy detaches the task from its release train, retargets it to the integration branch, and notifies the supervisor. To keep it on the train, record `keep release train: <train-id>: <reason>` as a supervisor decision before the threshold is reached. Keeping the task on the train does not waive review.
