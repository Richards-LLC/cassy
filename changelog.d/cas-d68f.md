### Added — Factory

- **Park names every symbol a delivery leaves without a caller** — Was: a
  refactor that dropped the last caller of a helper parked cleanly. The
  verifier then rejected it, one unused helper per round, at about 15 to 20
  minutes a round. Now: park checks the delivery diff in seconds, with no build
  and no model call. It refuses with one list of every TS/JS function, method,
  export or type, Rust `pub` item and CSS class the delivery left unreferenced,
  or added with no caller. Symbols that were already unused before the delivery
  are not listed. A class that markup builds from its prefix counts as used. A
  supervisor can waive a symbol that is only reached at runtime by overriding
  with a reason.
