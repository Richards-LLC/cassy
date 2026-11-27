### Fixed — Releases

- **Published releases keep their frozen QA after the merge queue** — Was: when the queue published a release as a merge commit, or a retried cut re-audited its prepared commit, the publication closeout dropped the audit and borrowed an older release review. Now: the closeout binds QA recorded at the train's frozen tip. An audit that does not identify the tag binds no proof. A held closeout resumes once when those bindings change.
