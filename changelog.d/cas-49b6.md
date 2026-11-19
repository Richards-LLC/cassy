### Fixed — Workers

- **Worker check no longer goes silent before build admission** — Was: a capped worker check classifying changed tests printed nothing until admission, so a long classification (16 minutes observed) looked hung. Now: it prints a "still classifying changed tests" line with elapsed time every 30 seconds until classification ends.
