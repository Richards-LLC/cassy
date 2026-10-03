# Independent QA cas-e0be / cas-d1fa

Pass qapass-9ccabce6b2cf5acf, round1. Reviewer wise-raven-87; implementer quiet-pelican-69.
Exact tip bd3d3afd4899373b8041d46bb7c69e23c99f5fc1, factory/quiet-pelican-69-cas-d1fa; base48c4f51a08cc388221c5b1bb2fd3d679da69b8df.
Independent npm run build exit0 in detached target/qa-e0be-fix/hub-web. Base compiled independently AFTER committed-dist RED proof; base-build.log exit0. No delivery source/branch changed. All servers Playwright webServer-owned, reuseExistingServer=false; fix journeys29129/base29130, keyboard29131/snapshots29132, repeatstrict base29133/snapshots29134; stopped on completion. Native macOS Chromium/Playwright1.63.0. HubDouble only supplies protocol boundary; real production canvas/focus/keyboard/accessibility rendering ran.
sweep: not configured

## Journeys

Full40pass/2fail of42, exit1/8.3minutes. Failures HUB-J3 find-conversation.journey.ts74 and HUB-J8 switch-machines.journey.ts93 reproduce on exactbase0/2: unchanged CtrlK assertions vs correct nativeMac CmdK. Remainders after those assertions NOT EXERCISED; no fullgreen claim. Same2 new HUB-J12 parts --grep cas-d1fa RED0/2 on committedbase dist, GREEN2/2 in ownbuiltfix/fullsuite. ObserverRED remainsfocused; controllerRED missinghint. Separate red-results and base-failures-results preserved.

| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HUB-J1 |PASS1|0|0|0|0|0|Note|journeys/HUB-J1/|
| HUB-J2 |PASS1|0|0|0|0|0|Note|journeys/HUB-J2/|
| HUB-J3 |FAIL samebase|—|—|—|—|—|Normal pre-existing|F10, F10-trace.zip28|
| HUB-J4 |PASS2|0|0|0|0|0|Note|journeys/HUB-J4/|
| HUB-J5 |PASS1|0|0|0|0|0|Note|journeys/HUB-J5/|
| HUB-J6 |PASS1|0|0|0|0|0|Note|journeys/HUB-J6/|
| HUB-J7 |PASS2|0|0|0|0|0|Note|journeys/HUB-J7/|
| HUB-J8 |FAIL samebase|—|—|—|—|—|Normal pre-existing|F10; full.log/base-failures.log|
| HUB-J9 |PASS1|0|0|0|0|0|Note|journeys/HUB-J9/|
| HUB-J10 |PASS1|0|0|0|0|0|Note|journeys/HUB-J10/|
| HUB-J11 |PASS1|0|0|0|0|0|Note|journeys/HUB-J11/|
| HUB-J12 |PASS24|0|0|0|0|0|Note|journeys/HUB-J12/;2newparts|
| HUB-J13 |PASS2|0|0|0|0|0|Note|journeys/HUB-J13/|
| HUB-J14 |PASS1|0|0|0|0|0|Note|journeys/HUB-J14/|
| HUB-J15 |PASS1|0|0|0|0|0|Note|journeys/HUB-J15/|
| HUB-J16 |PASS1|0|0|0|0|0|Note|journeys/HUB-J16/|

## Correctness

8 exploration cells/2 cases PASS in1.6m;54 passing Expect events/zero runner errors (27 pertrace). Maintrace.zip390, trace-1280.zip desktop. Keyboard-only primary actions; tabTo uses native Tab/document.activeElement, no .focus()/mouse substitute.

| Cell | Goal | Result | Evidence |
| --- | --- | --- | --- |
| F01 |1280 keyboard Terminal/input |PASS visible hint and exact accessible description|F01.png; trace-1280.zip|
| F02 |1280 programTab/CtrlM/Enter |PASS bytes9/13 and real nextfocus|F02.png; facts-1280.json|
| F03 |1280 longoutput/schemes/media |Modes PASS; manualoverlay FAILF09|1280-light/dark.png; a11y captures|
| F04 |1280 revoked observer exit/Re-pair |PASS Tab/ShiftTab leave, keyboardEnter opensPair|F04.png; trace-1280.zip|
| F05 |390 keyboard Terminal/input |PASS exact description|F05.png; trace.zip51–53|
| F06 |390 programTab/CtrlM/Enter |PASS bytes9/13, realnextfocus|F06.png; trace.zip61–69|
| F07 |390 fulloutput/media |Modes PASS; manualoverlay FAILF09|390-light/dark.png; trace.zip107/110/127|
| F08 |390 observer/Re-pair |PASS entirelykeyboard Pairdialog|F08.png; trace.zip137/172/201–203|

aria-describedby resolves full sentence “Tab goes to the terminal. Ctrl+M leaves it.” Real computed accessible-description assertion and controlled-1280/390.aria.json are included; this proves screen-reader-facing tree, not an audible VoiceOver session. Forced-colors/reduced-motion/more-contrast matchMedia true asserted and screenshotcaptured. FinalARIA is Pairdialog after keyboardrecovery, controlledARIA holds hint.

## Introduced findings

F09 HIGH: New hint hides liveprogram text at bottom while focused/incontrol. styles.css1178–1190 sets absolute/right8/bottom8/z3 without reserving terminal geometry. Actual wire Output fills40rows: phone row40 hiddenx121–373/y646–671; desktop rightpart row40 hidden. Identical base screenshots show unobstructed text. F09.png (=390-dark.png), trace.zip screenshot107(light)/110(dark); desktop1280-dark.png/trace-1280.zip screenshotactions; baseline/390-dark.png and baseline/1280-dark.png. Real canvas, not mocked image. Reserve external footer and resizecanvas. Craft2<3 is explicit rejection bar independent of contrast uncertainty.

F12 NOTE: CtrlM works in tested Chromium; Enter still sendsCR13, matching surface.ts364 comment recognizing CtrlM as carriage-return. Mozilla documents CtrlM as browsermute: https://support.mozilla.org/en-US/kb/keyboard-shortcuts-perform-firefox-tasks-quickly and https://support.mozilla.org/en-US/kb/mute-sound-firefox-tabs. Documented Firefoxconflict, no runtimeFirefox failure claimed. Recommend configurable CtrlAltM excluding AltGraph plus visible Leave terminal action, then browser/OSmatrix; proposed alternative not claimed universally free of conflicts. F12.png, trace.zip51–53 hint/61–65 currentexit. Does not alone reject Chromium delivery.

## Polish

# Independent critique: Terminal keyboard exit

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness |4|Existing Cassy terminal, machine identity and control badge remain coherent.|
| Fit |4|Hint accurately distinguishes program Tab from browser exit; observer navigation works.|
| Hierarchy |4|Small hint subordinate to program/control header.|
| Craft |2|NEW absolute bottom-right hint obscures program's last row while focused, desktop and phone. Base identical output unobstructed. F09, trace.zip107/110; base1280/390-dark.png. Reserve footer space outside canvas and resize terminal.|
| Accessibility |4|27 passing runner Expect events per viewport; exact accessible description; Tab9, Enter13, CtrlM exits; observer Tab/ShiftTab/Re-pair;3 media queries true. FirefoxCtrlM conflict is a portability note.|

Changed-path Craft2 fails floor3. No overall strict PASS: existing100ms CSS color transitions make inspector50ms captures unstable. Raw contrast deltas recorded without attributing all to this change. Functional keyboard criteria pass.

Strict sameinspector/allowlist/ownlocalbuild20renders pertip/base. Finalrepeat18tip/11base findings, raw normalizeddelta11 in baseline-comparison.json. Initial1tip/3base already varied. Old styles.css43 transitionscolor/background100ms; visual-qa.mjs940 settles50ms. Intermediate colors cause unstable contrast reports. Do not claim scoped0new or overallPASS; do not causally attribute all transient deltas to this sourcechange. Rejection rests on stable reproducedcanvasobstruction. Noallowlist added, inspector unmodified.

## Pre-existing / limitations

F10 NORMAL: nativeMac CtrlK fixture mismatch, sameexactbase. F10.png extractedfrom realHUB-J3receipt at3sec; F10-trace.zip28/F10-actions.txt; full.log/base-failures.log. Existing followupcas-2a33. Laterpaths blocked byassertionsnotexercised.
F11 NORMAL: strict contrast captureinstability and oldbackgroundcontrols confirmedonbase; visual-qa/visual-qa.json, visual-qa-baseline/visual-qa.json, baseline-comparison.json. F11.png; trace.zip107 samecapturedterminalcontext. Controls include codename, Interrupt and Conversations; nohintcontrastfinding. Suggest settledstyles/animations before attributing rawdeltas. Rawreport18vs11 remains recorded, not silently waived.
Initial28row outputprobeleftblankbottomandmissedoverlay. Correctedartifactharnessonly to40rows/repeatedtext, reranbothwidths andsamebase. Baselinerunneruses softexpectations preserving expectedREDstates:0pass2fail/exit1. Tip54assertions pass; manualcanvasfinding separate. Strictrepeat replayedsamecapturedHTML/realbuiltassets, canvaspixels evaluatedonlyin realinteractioncaptures (HTMLsnapshotsdo not serializecanvas).
No source edits/commit needed for no-code review; no live machine or Slack actions. All4 previews stopped. Verdict REJECT exactbd3d3afd4: introduced terminal-textoverlay/Craft2; functional escape+description pass, narrow rework requested.
