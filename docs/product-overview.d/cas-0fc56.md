## Core Concepts

A release train runs its full suite once, when it is frozen for release. Merges into an unfrozen train start no train suite, because lane CI already proved each delivery. The daemon's train CI scan dispatches the shared full-validation workflow once per frozen tip; a repair or plumbing commit on a frozen train is a new tip with its own single run. The cut consumes that run's green receipt, and a red raises the train-red notice. Auto-cut freezes a ready train with changelog fragments, asks for notes while the suite runs, and cuts on green.
