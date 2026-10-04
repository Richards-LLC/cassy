# Brief: Terminal pane activity caption (cas-846c)

## Single idea

The pane name and keyboard controls keep their space; an activity caption reads in full when it fits and is absent from both the painted line and accessibility tree otherwise.

## Hero form

The existing ruled Terminal header carries the claim: name and controls stay whole, with one quiet activity phrase occupying only the remaining space.

## Emotional register

Quiet and precise: keep the current muted ink and type tokens, and remove the misleading clipped reading rather than introduce another status indicator.

## Distinctive move

Use the actual laid-out text bounds, including fractional clipping, to decide whether the entire caption fits. Keep its tooltip and reserved flex slot so a shorter label or wider header can restore it.

## Deliberately omitted

No ellipsis, generic phone-width suppression, new color, navigation change, activity-clock rewrite, or checker/allowlist exemption. The two twin pane names and Leave keyboard control retain their current priorities.

## Cause and source boundary

Matched RED art-852ef2b5 preserves head/base six identical strict findings: `Earlier output`, client26×17/scroll26×34. Existing `styles.css:961–978` pushes a whole anonymous text flex item below the clipping box; `main.ts:updatePaneActivity` still leaves that hidden text in the reading DOM. Correct only caption presentation and its measured refresh lifecycle. Do not import the unlanded S6 branch or alter its async ownership.

## Source-first regression contract

- Detached/hidden creation must recover after the real card is placed; output/keyframe updates must replace the stored label and tooltip.
- Below-line and fractional clipping must leave no reading text and set `aria-hidden`; fitting text must remain whole and accessible.
- Resize/drawer re-render/font completion must restore a previously hidden long label when it fits. A live output event changes the long label to `now`, which must appear if it fits.
- Real committed phone build at390×844 and844×390: capture long/short states, reverse resize, drawer changes and actual font completion; assert name/key/controls geometry, no clipped caption reading/AX residue, normal pointer and keyboard Terminal return, and input focus stability.
- Run the same unchanged strict command on exact base and corrected captures in light/dark; retain original six+six failures and fresh exact source/dist/runner/cleanup binding. No fullcatalog or real backend/audit claim.

## Critique

NOT EXERCISED: supervisor313219 allows source preparation only. Scores and real layout/AX/strict evidence follow explicit build/browser/server admission; unit geometry contracts do not substitute for browser proof.
