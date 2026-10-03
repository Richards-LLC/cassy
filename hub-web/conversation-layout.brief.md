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
Pending matched production builds and strict QA.
