# Hub mobile study QA

Build: aaa49f0fde8360402b03c3a40b24be55a33914c5
HTML SHA-256: 91c04740f890b81ddac0a397a1658c4a02c4ca3bf3ff31be61562e975ad97e01
Scope: Open, zoom, pan, fullscreen and rotate the committed offline design study.
Surface: docs/design/hub-mobile/index.html
Budget: 30 minutes
Cells: 8; PASS: 8; FAIL: 0; NOT EXERCISED: 0
Labels: real-build 8
sweep: not configured

id | cell | expected | observed | verdict | label | evidence path | defect task
--- | --- | --- | --- | --- | --- | --- | ---
M01 | Phone image → pinch → fullscreen → landscape → scroll | I can zoom, fill the screen, rotate and keep reading at the same relative place. | Touch pinch 1.69×; fullscreen native; scroll ratio 0.400 → 0.400 on rotation; drag continued scrolling. | PASS | real-build | M01-rotated-reader.png | none
M02 | Appearance and geometry at six scheme/viewport combinations | The study and image controls stay readable without page overflow in light and dark. | All 6 viewport/scheme pairs: 10 proposals, 20 proposal views, no horizontal page overflow; reader controls visible and vertically scrollable. | PASS | real-build | matrix-390x844-light.png | none
M03 | Fullscreen API rejects; page and image fallback | Full screen still gives me a scrollable full-viewport reader and Escape restores the page. | Native API deliberately rejected; CSS viewport fallback remained scrollable after rotation. First Escape exited fullscreen; second closed the reader. | PASS | real-build | M03-fullscreen-fallback.png | none
M04 | Keyboard-only opening, zoom, scroll, close and return | I can operate the image reader with the keyboard and focus returns to the image I opened. | Enter opened; + zoomed; PageDown scrolled; 0 fit width; Escape restored focus to the opening image. | PASS | real-build | M04-keyboard-reader.png | none
M05 | Revisit and switch images after zooming | A new image opens at fit width; the correct caption and system context follow it. | Reopening a different image reset to fit width and showed its own caption; mouse wheel zoom increased the new image scale. | PASS | real-build | M05-revisit-desktop-image.png | none
M06 | JavaScript disabled and print | All ten proposals, baseline images and the comparison remain available. | JavaScript off: 10 proposals, 10 comparison rows and all 25 images decoded; print PDF contains the study. | PASS | real-build | M06-no-javascript.png; /home/pippenz/.cas/artifacts/cas-cc1b/qa/study-print.pdf | none
M07 | Offline standalone file and all embedded images | The study opens from a local file and loads every image without network access. | Opened standalone file with browser offline; zero HTTP(S) requests; all embedded images decoded and current-state lightbox opened. | PASS | real-build | M07-offline-current-image.png | none
M08 | Whole-study fullscreen and comparison navigation | I can read the entire study fullscreen, reach the comparison and return without losing my place. | Whole-study fullscreen (native) reached the comparison; Escape restored document scrolling near the same position. | PASS | real-build | M08-study-fullscreen-comparison.png | none

## Constants vs expectation
| constant | location | visible contract | predictable? | defect task |
| --- | --- | --- | --- | --- |
| MIN_ZOOM=.25, MAX_ZOOM=5 | reader.js | Visible zoom output; Fit width resets scale | Yes: bounded zoom; no timeout | none |

## Contradictions
No contradictory end-state claims observed. Drawings say proposed; baseline captions distinguish live and simulated systems. Fullscreen fallback names browser API unavailability.

## Honesty
- Browser automation exercises the actual HTML, not a mock of its reader. Chromium touch emulation and viewport resizing are not a physical phone or OS rotation. Native mobile Safari remains untested.
- M03 deliberately rejects requestFullscreen to exercise browser failure recovery.
- Initial capture experiments with restored browser storage lost authentication; baseline capture instead paired and captured in the same context. Those failed experiments are excluded from the baseline.
- A first preview screenshot preceded lazy image decoding; final evidence waits for decoded images.
- Proposed layouts are drawings. Their depicted product interactions and design impact estimates have not undergone user validation.
- Six scheme/viewport pairs are captured separately in results.json. Print and no-JS artifacts are included.
