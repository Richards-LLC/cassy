## Core Concepts

- **Operations-only epic close** - an epic whose children are all terminal, with no code delivery on the epic or any child, and whose epic verification is approved closes through ordinary `task action=close`, including when its branch has no structural delivery range (for example, a repository's root commit). The close records an audit note naming the approved verdict and child count. Any child code delivery keeps the pre-close structural lint and its refusal.
