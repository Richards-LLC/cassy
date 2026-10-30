# Brief: native recording chapters

## Single idea

A recorded cell title stays readable over native Commander without changing the
page's content security policy.

## Hero form

A single centered caption identifies the next interaction; the conversation
remains visible around it at desktop and phone widths.

## Emotional register

Quiet and temporary: the page's own surface, ink and accent tokens, no backdrop
blur, and no movement that could distract from the recorded application.

## Distinctive move

A small accent rule ties a numbered cell caption to Commander's visual language.
The annotation cannot take focus or intercept a click.

## Deliberately omitted

No inline stylesheet, HTML title interpolation, production asset or CSP exception.
The caption is a JavaScript-only recording aid, hidden from accessibility trees
and print, and removed before the application stage begins. Long titles wrap
inside the viewport; unusually tall captions scroll within their own box.

## Critique

| Dimension | Score | Native evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Commander's own surface, type and accent rule identify the recording caption. |
| Fit | 5 | A frame extracted from the actual recording shows the cell title over native Commander. |
| Hierarchy | 4 | One centered title precedes the recorded stage; the conversation remains recognizable. |
| Craft | 4 | Phone and desktop captures in both schemes fit the viewport; a long literal markup title wraps without clipping. |
| Accessibility | 4 | Caption contrast is 17.25:1 light and 14.76:1 dark; focus, typing and clicks remain with Commander, and the caption clears afterward. |

The native recording probe checks caption bounds, text overflow, pointer input,
cleanup and zero style-policy events. It does not claim whole-page visual polish
certification. The annotation requires JavaScript, introduces no animation and
does not render in print.
