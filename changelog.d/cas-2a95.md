### Fixed — Commander

- **One reply per supervisor answer** — Was: when a Stop hook blocked a
  supervisor's draft, Commander showed the blocked draft, and sometimes the
  rewrite too. Now: a Claude answer reaches Commander only after its Stop hooks
  accept it, so the operator sees the accepted rewrite once (GH #1938).
