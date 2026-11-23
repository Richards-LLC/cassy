### Fixed — Task close

- **Close evidence citations ignore surrounding sentence punctuation** — A close reason that cites a proof log in prose, such as `(target/run.log).` or a path ending in a period, now publishes the existing file instead of refusing it as missing. A path in backticks is read exactly as written, and a file whose own name ends in punctuation, such as `run.log!`, is published as itself rather than as a similarly named sibling.
