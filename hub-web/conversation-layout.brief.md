# Brief: Conversation mount geometry

## Single idea

Reading a phone conversation uses the full phone width without inheriting the hidden terminal's 80-column pan.

## Hero form

The existing conversation thread remains the reading column, with the latest reply above the composer and the terminal available through Terminal view.

## Emotional register

Quiet and predictable: keep the existing Pebble paper, typography, focus ring and message hierarchy.

## Distinctive move

The real terminal stays mounted with its backing grid while its hidden descendants leave the conversation's scroll geometry; returning to Terminal view restores the full grid.

## Deliberately omitted

No checker exception, new clipping rule, palette, dimensions or remount. A visible conversation mount remains subject to strict overflow inspection.

## Diagnosis and scope

The committed base at dacbfab93 reproduces scrollWidth634/clientWidth390 in both schemes. The actual reading thread is390/390 and receives pointer hits. Removing the hidden canvas from layout alone changes mount scrollWidth to390 without changing reading geometry. Ghostty fit measures the mount, and conversation mode already suppresses canvas painting. The CSS removes hidden descendants from layout only while conversation-active is present.

## Critique

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Distinctiveness | 4 | Existing Pebble reading column, machine identity and quiet timestamps remain intact. |
| Fit | 4 | The phone reader fits390px, while Terminal view retains its634px grid and horizontal pan. |
| Hierarchy | 4 | Replies remain above the composer; hidden terminal chrome contributes no reading geometry. |
| Craft | 4 | Matched strict16renders remove all4 original findings without a checker/allowlist change; long content wraps. |
| Accessibility | 4 | Reading hit tests, return-control Enter/composer focus, keyboard send, and all3 media modes pass. |

Matched real-build/protocol-double matrix covers reading, long dark reply, Terminal view/return, desktop/theme resize, outage recovery, list revisit, keyboard send and media modes. Initial base strict4 findings reproduce the original634/390 overflow; corrected strict0. The backingstore stays634 while conversation geometry becomes390/390. Full physical-device/backend/all-platform evaluation belongs to supervisor assembly; no claim follows from these local browser proofs.
