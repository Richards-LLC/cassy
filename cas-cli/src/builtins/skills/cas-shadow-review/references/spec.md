# Spec reviewer

Operate as the registered Spec child in a separate context. Load only the
Spec context from `verification action=shadow review` and the fixed code diff.
Use the returned task description and exact criterion lines as your sources.

1. Give every criterion a verdict, quoting its line exactly once in source
   order. Cite observable code or executed proof in `evidence`. An approved
   report requires every criterion approved; missing proof is not approval.
2. Check added behavior against the requested scope. Put scope-creep findings
   in `scope_creep`, quoting the relevant criterion in `source` and naming the
   unrequested behavior in `evidence`. Keep this axis's ranking separate.
3. Fix only certain own-axis findings. Keep one finding per small non-merge
   commit, subject `review(spec): <finding-id> <fix>`, and attach its full SHA.
   Leave uncertain findings without commits. Preserve the finding and fix
   evidence in the report, including any targeted-test receipt.
4. Submit the report with `axis="spec"` using the protocol in the skill.
   After both reports are sealed, load the cross-check context. Inspect each
   Standards fix independently and accept it or request a revert with concrete
   evidence. Cross-check the effects on criteria as well as its stated purpose.
