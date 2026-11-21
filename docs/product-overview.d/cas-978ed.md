## Core Concepts

A parked delivery merges through an isolated merge that consumes its exact
anchor and leaves the worker's checkout untouched. Its source proof depends on
the delivery mode. A `push_branch` delivery needs a freshly fetched origin ref
that matches the anchor. A `local_merge` delivery is proven by its local branch
at the anchor alone, so it merges even when origin is unavailable.
