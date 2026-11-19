### Added — Release

- **The next release train syncs with main by itself** — Was: after a release landed, the next train stayed behind main until someone merged by hand; the Nov 12-13 manual sync hit 14 conflicts and cost a night, and a spawn only found out through EPIC BASE REFRESH REFUSED. Now: the daemon merges the integration branch into every open, unfrozen train within one CI poll and publishes the lane, then tells workers with unfinished deliveries on it; a conflict changes nothing and files one sync task under the train naming the conflicting files, with one notice to the supervisor.
