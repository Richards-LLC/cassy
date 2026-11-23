## Core Concepts

A changed Playwright helper with no tests of its own is proved through the specs that import it. Worker-check runs each importing spec under the Playwright config that lists it, and park accepts the helper only when every importing spec has a passing capped run at the tip. A spec run on its own under its config counts the same way; a missing or unrelated spec does not.
