# Brief: composer uploads

## Single idea
The composer shows exactly which pasted files will accompany the next message.

## Hero form
Annotated strip: a compact thumbnail, filename and remove control sit immediately above the familiar message field, so the next send's contents are visible together.

## Emotional register
Calm and concrete: existing Glass panel, ink and rule tokens; errors remain beside the file that needs attention.

## Distinctive move
Long pasted text folds into a readable text preview in the same attachment strip as screenshots, without stretching the conversation composer off the phone.

## Deliberately omitted
No extra attachment dialog or upload progress overlay: the strip stays in the single composer and Send owns the upload.


## Critique

Design source: `hub-web/DESIGN.md`, existing Glass tokens and composer clip. Checked at source81b9fa9d8, 1280×800 and390×844 in both schemes.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The existing clip opens the native picker; long pasted text folds into the same strip as screenshot thumbnails. |
| Fit | 5 | Existing panel, ink, rule and action tokens; composer stays the single bottom bar. |
| Hierarchy | 4 | Bounded previews sit above the field; filenames and Remove remain distinct; the close-page warning stays outside the scrolling file list. |
| Craft | 5 | Four strict visual-QA PASS receipts with zero findings; no horizontal overflow, clipping or overlap. |
| Accessibility | 5 | 44px Remove controls, explicit names, polite file status and token contrast in light and dark. |

Evidence: task artifacts `iteration3/journeys/HUB-J24/` contains real rendered states and ARIA; `visual-qa/{1280,390}-{light,dark}/visual-qa.json` checks frozen DOM from those states with production CSS/fonts embedded. The first frozen capture's file-origin CSS was refused by CORS; it is retained under `visual-qa-invalid-css-cors` and does not count as product evidence.
