## Core Concepts

- **Repair class audit** - A review verdict that sends a delivery back for failing tests carries `class_audit`, a nextest filter for the whole test class, alongside `tests_failed`. Cassy stores it on the task and the worker's next park needs a passing capped receipt at the tip that runs every part of that class. A verdict without failing tests needs neither field, and a later verdict without a class clears the earlier one.
