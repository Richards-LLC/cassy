## Primary Journeys

A green release-train CI result checks the current train state and containing version tags before asking the supervisor to cut. Shipped or closed trains produce no new cut-ready notice; an unreleased train still notifies once when it first turns green or recovers from red.
