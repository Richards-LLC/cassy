# Slack draft — release report evidence: issues, publication, verdict, themes (main merge, PR #794)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: the generated release report could not see the issues a release closed, said the release was unpublished when it was live, and opened with a generic line. Now: it counts the closed issues, shows the publication time, opens with the release's own headline, and groups changes the way the release notes do.

Reply:
• Closed issues counted — Was: an issue named in the changelog or in the release pull request did not appear in the report's ledger or change map. Now: those references are collected, verified on GitHub, and counted under the matching theme.
• Publication shown — Was: the report header said "publication evidence unavailable" even for a live release with a receipt. Now: it reads "Published <date> · <time> UTC" from the release or its receipt.
• Headline as verdict — Was: the release notes' opening line became one more article. Now: that line is the report's verdict, capitalized and punctuated, and the two thread headers are not repeated as articles.
• Themes from the notes — Was: the change map used a fixed list of generic surfaces. Now: it takes the release notes' own section labels, assigns each closed issue to the section that mentions it, and records the themes so a re-run gives the same map.

## Dev thread

Top-level:
Live on production — Dev — Was: `cas release report` read only `closingIssuesReferences`, took the publication line from release metadata alone, rendered the draft's top-level as an article, and used the fixed `THEME_ORDER`. Now: PR body issue refs are extracted, `PUBLISHED_AT` from the receipt feeds the header, the draft punch becomes the normalized verdict, and themes derive from the draft's bold groups and persist in front matter (PR #794).

Reply:
• Issue discovery — Was: `collect_pr_issue_references` read `closingIssuesReferences` only. Now: PR body references are extracted too, merged with changelog references, verified through `gh issue list`, and assigned to the theme of the draft article that mentions them.
• Publication header — Was: `publication_line` consulted release metadata only. Now: `PUBLISHED_AT` from `release-published.receipt` or the release object renders "Published <date> · <time> UTC".
• Verdict and sections — Was: top-levels treated as articles, bold group labels discarded. Now: the User punch becomes the verdict (capitalized, terminated), both top-levels are skipped, sections use the draft's bold group labels.
• Themes — Was: fixed `THEME_ORDER`. Now: derived from the draft's groups at assembly time and written into the front matter.

Measured on v3.22.0 sources: Issues closed = 1 (#767 under Verification), themes Release / Memory / Verification / Factory, header 16:51 UTC. Proof: unit 13/13; release_report_test 2/2; terminal-qa PASS 12 runs. PR #794.

## POSTED
Posted 2026-09-09 18:24Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788978257.945319 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788978257945319 (reply ts 1788978270.103489)
- Dev top-level ts 1788978272.562369 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788978272562369 (reply ts 1788978296.695599)
