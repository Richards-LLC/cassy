# Delivery handoff

The developer sends two messages after the round's report is committed.
Both are drafted into
`docs/sow/<sow-slug>/reports/<date>-<sow-slug>-round-<n>-handoff.md`.
Neither is posted until the operator gives an explicit go. Post through
`violet` and attach the PDF to both messages.

## Slots

| Slot | Value |
| --- | --- |
| `{pm_mention}` | `DELIVERY.md` → `pm_mention` |
| `{client_name}` | `DELIVERY.md` → `client_name` |
| `{sow_title}` | Title from the SOW document |
| `{env}` | `staging` or `production`: the environment every screenshot came from |
| `{round}` | This delivery's round number (1 = first delivery) |
| `{rounds_included}` | `DELIVERY.md` → `rounds_included` for this SOW's size |
| `{review_window}` | `review_cycle_days` written as words a client reads, e.g. "2 week" for 14 |
| `{met}` / `{total}` | Met rows / all rows in `GAP-CHECK.md` |
| `{open_decisions}` | Each Partial row's client decision, one per line, or "none" |
| `{report_file}` | PDF file name |
| `{round_previous}` | `{round}` − 1, for a later round's first sentence |

## 1. Developer → PM (`internal_channel`)

> {pm_mention} I believe {sow_title} is complete. {met} of {total} outcome
> requirements are met on {env}. Open client decisions: {open_decisions}.
> Round {round} of {rounds_included} report attached ({report_file}).
> The client message is below for you to send or for me to post with your OK.

## 2. Client message (`client_channel`, tags the PM)

This wording is the agency's standard phrasing. Keep it verbatim apart from
the slots. If the client still has open decisions, add one line after it
listing them; the closing promise stays the same.

> We've closed those last gaps that we were discussing and we believe this SOW
> is now considered complete. We have deployed these latest changes (on {env})
> and I have attached a report so you can follow the latest changes. This is
> round {round} of {rounds_included}. To keep the project moving please review
> as soon as convenient so we can stick within the {review_window} max review >
> signoff cycle. Please bundle your changes/tweaks into a 'round' of changes as
> this prevents too much context switching for our team and it's better for you
> to review in a round of changes. {pm_mention}

For a later round, replace the first sentence with "We've completed the
changes from round {round_previous} and closed the gaps we discussed."

## 3. Status to PM (`internal_channel`, status variant only)

Use this instead of messages 1 and 2 when `check-sow.py gap` stopped on Gap
rows. It goes to the PM alone and never claims completion.

> {pm_mention} Status on {sow_title}: not yet complete. {met} of {total}
> outcome requirements are met on {env}. Still ours: {gaps_ours}. Waiting on
> you, because the SOW is ambiguous: {gaps_pm}. Questions drafted for the
> client: {open_decisions}. Status report attached ({report_file}); it is
> for you, and goes to {client_name} only if you decide so.

`{gaps_ours}` lists the `Gap` rows and `{gaps_pm}` the `Gap → PM` rows, each
as `R#` and its short name, or "none".
