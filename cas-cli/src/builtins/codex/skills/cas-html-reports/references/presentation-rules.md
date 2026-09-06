# Presentation rules — charts, tables, numbers

These rules are derived from the IBCS notation principles (see `sources.md`) and adapted to Cassy
reports. They apply to every report type; the financial and executive types enforce them hardest.

## The governing principle

**The argument is visible; the notation keeps it legible.** The hero figure named in the concept brief
shows the claim before any prose. Everything else on the page serves that figure: evidence twins,
marginal notes, the closing figure that carries the ask.

**Same things look the same, different things look different** is the constraint that makes the
argument readable, not the goal. Within a report and across successive editions, a series keeps its
color, a scenario keeps its fill, a status keeps its glyph and position, and a reader who learns the
notation once never relearns it. A report that satisfies this constraint and has no hero has failed.

## Form vocabulary

Take the form from `cas-ui-craft` and write the reason in the brief. The forms a report reaches for
most, with the reader task each one fits:

| Reader task | Form | The reason it fits |
| --- | --- | --- |
| See a claim next to what contradicts or supports it | Ledger: two aligned columns per row, a verdict stamp in the gutter | Contradiction is spatial; the eye reads across |
| Compare several similar series | Small multiples on one shared scale | Differences carry the work, not chart furniture |
| Show change between two states | Slope or dumbbell | The delta is the mark, and direction is the shape |
| Show when things happened relative to each other | Annotated timeline | Causality is an ordering claim |
| Show a count as parts of a whole | Waffle or dot plot | Every unit is visible; nothing is hidden in a wedge |
| Carry the one sentence that must survive | Pull-quote set in the display face | Typographic weight is hierarchy |
| Attach a caveat to the evidence it qualifies | Marginal note beside the row or mark | Proximity beats a footnote |

## Scenario encoding (actual / plan / forecast)

Fill carries the scenario; color carries the series. Never the other way around.

| Scenario | Fill | Notes |
| --- | --- | --- |
| Actual (measured, closed) | Solid | The default |
| Plan / budget / target | Outlined, no fill | Same color as the actual it plans |
| Forecast / projection | Hatched or dotted pattern | Always labeled as forecast in text too |
| Prior period | Solid, lighter tone or a thin reference marker | Never confusable with actual |

An SVG `<pattern>` gives you the hatch with no dependencies. Because the encoding is by fill, it
survives grayscale printing — which is the point.

## Variance first

For any comparison, the delta is the message.

- Show **absolute variance** and **percent variance**, in that order, adjacent to the base value.
- A variance chart is a signed bar centered on a zero line — not two value bars for the reader to subtract.
- Favorable and unfavorable are distinguished by **direction and pattern**, and only additionally by
  color. Label the sign convention explicitly: "positive = above plan".
- State the comparison base every time: "vs plan", "vs prior quarter", "vs baseline commit `abc1234`".
  A percentage with no named base is meaningless.

## Scales and axes

- **Sibling charts share a scale.** If two charts sit side by side and invite comparison, their value
  axes must be identical, or you have drawn a lie.
- **Bar charts start at zero.** Always. If the interesting variation is small, plot the variance instead
  of truncating the axis.
- **No dual axes.** Use two stacked charts sharing an x axis.
- **No 3-D, no perspective, no donut holes with numbers in them.**
- Time runs left to right. Categories are ordered by value unless a natural order (time, severity,
  stage) exists — then use it, consistently.

## Chart choice

| Message | Use |
| --- | --- |
| Value over time | Line (continuous) or column (discrete periods) |
| Structure / share of a whole | Stacked bar, ≤5–6 segments; merge the tail into "Other" |
| Ranking across categories | Horizontal bars, sorted |
| Variance vs a base | Signed bars on a zero line |
| Distribution | Histogram or box plot; never a bar of means alone |
| Two-variable relationship | Scatter, with the correlation stated in text |

If the message is a single number, do not draw it — write it large as a KPI card with its variance.

## Legends and labels

- **Label the data directly** where you can: value at the end of a bar, series name at the end of a line.
  Direct labeling beats a legend every time.
- A legend appears **only when direct labeling is impossible**, and then it goes **below the chart,
  centered**, in the same order as the series appear.
- Every chart states its **unit** in the axis title or the chart title — never leave a reader to infer
  whether that is dollars, thousands, or percent.
- Chart titles state the message, not the mechanics: "Spend ran 12% under plan", not "Spend by month".

## Tables

- **Numbers right-aligned**, text left-aligned, headers aligned with their column's data.
- **Consistent decimal places** within a column. Do not mix `3.1` and `3.14` in one column.
- **Sums and totals bold**, with a rule above. Subtotals visually subordinate to totals.
- Negative numbers in one consistent notation — parentheses or a leading minus. Pick one per report.
- Units and currency in the column header, not repeated in every cell.
- Highlight the one value per column that carries the message (best, worst, breach) — with weight or a
  marker, and a text label; not with color alone.
- Sort by the column the reader cares about, and say which.
- Large tables: keep the header visible (`position: sticky` on screen, `table-header-group` in print)
  and cap what is shown, linking to the full data file rather than paginating with JS.

## KPI cards

Every card carries four things: the **value** (largest), the **label**, the **comparison base**, and the
**variance** (absolute and percent, signed). A card without a comparison base is a decoration.

Trend direction is shown by an arrow glyph *and* the sign, never by color alone. Three to five cards;
past five, nobody reads any of them.

## Numbers in prose

- Round consistently and say so once. Precision beyond the measurement's accuracy is a false claim.
- Percentages: state whether a change is percentage points or percent — "up 3 points" vs "up 3%".
- Big numbers get thousands separators; sample sizes are always given for any rate or average.
- Never state a rate without its denominator.
