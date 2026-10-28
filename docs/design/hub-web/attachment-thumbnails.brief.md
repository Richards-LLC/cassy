# Brief: attachment thumbnails

## Single idea

An image is recognizable inside its own file card, and a failed preview leaves a compact file card with its own explanation.

## Hero form

A labelled file row: a reserved 48px real thumbnail sits beside that file's name and size, so the pixels and caption share one clickable boundary.

## Emotional register

Calm, practical — existing Commander sheet tokens and typography, restrained thumbnail size, and one quiet inline failure note.

## Distinctive move

Replace the red image type plate with the actual served image while retaining Commander's folded file sheet and the existing full-preview action.

## Deliberately omitted

No separate full-width image above another file's caption, no new viewer, and no empty failure panel. SVG/HTML never become thumbnails; non-images retain their type chip and supported media keeps its existing playback/preview.

## Implementation contract

The shared inline attachment renderer owns card layout and failure notes; the existing media lifecycle owns ticket acquisition, URL validation, decode failures and reconnect retry. Thumbnail space is reserved before lazy code or bytes arrive. Fresh rendering/reload obtains served bytes from the artifact reference, without object URLs or upload-only state. The entire file link remains keyboard accessible and opens the existing preview.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The existing folded Commander sheet now carries actual image pixels in its type plate, retaining the sender silhouette. |
| Fit | 5 | Thumbnail, name and size share one native file link; failed content reduces to that file's note. |
| Hierarchy | 5 | A 48px thumbnail identifies its own file, with an 8px separation from the next file and no competing full-width preview. |
| Craft | 4 | Strict visual QA passes eight success/failure captures at 1280 and 390 in both themes with zero findings. |
| Accessibility | 4 | Native named links remain keyboard reachable; thumbnail dimensions are reserved before lazy loading, failure notes are live, and both schemes pass contrast/layout checks. |

Scored by clever-lion-26 on 2026-10-28. Fixtures exercise the shared renderer; production-bundle journeys cover send, reload, second device, full preview and deliberate HTTP404 failure. Physical devices and production hosting are outside this task.
