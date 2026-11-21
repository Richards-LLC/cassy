## Core Concepts

A parked delivery that lands on its recorded target outside `worktree_merge`
(through a PR, a manual merge or a promotion, into any branch) is reconciled by
the factory daemon. Cassy records the target tip that contains the delivery,
then runs the ordinary close. An existing approval closes the task. With no
verdict, the close starts the post-merge verifier on the contained delivery
(at most four at once), and its approval closes the task. A gate that holds the
close goes to the supervisor once. Deliveries not yet on their target keep the
pre-merge approval requirement.
