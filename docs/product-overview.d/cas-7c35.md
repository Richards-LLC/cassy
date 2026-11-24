## Primary Journeys

When a worker parks a task, Cassy requires an evidence file only when the close reason cites a concrete file; ordinary words such as "target" never make a park wait on a missing file. Copies of artifacts fetched from Cloud are never published again, so evidence folders no longer grow on each park. A retried park replaces its earlier evidence summary in the task notes instead of adding another one.
