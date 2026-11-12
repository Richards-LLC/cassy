## Core Concepts

Restarting a reviewed release-sync task recognizes commits already reachable from origin's trunk or versioned release tags as shipped upstream. Those imports do not consume the lane's attribution history limit. Unshipped commits naming other tasks remain refused even when a prior delivery anchor contains them.
