## Core Concepts

A capped Playwright check for a shared test helper accepts setup tests from projects declared as dependencies of the projects running the required specs. Setup tests must pass but do not count toward the related receipt. An unrelated test or a missing required spec still prevents the receipt.
