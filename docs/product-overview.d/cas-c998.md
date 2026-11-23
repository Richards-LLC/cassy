## Core Concepts

A merge retry after a failed pre-merge candidate preparation reports that failure again. The retry resolves the same source, target and context as the first attempt, and each failed attempt sends its own failure notice to the supervisor, so a held merge never waits on a background preparation that has already ended. A successful preparation still reports one ready notice, and publication still requires the exact review.
