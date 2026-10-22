# cas update phase spacing

| Field | Contract |
| --- | --- |
| First two lines | Each update phase names its work and shows a separate working or completed status. |
| Scannable | The mark, counter, label, status and elapsed time remain distinct columns, with at least one space after every label. |
| Readable | A label that fills or exceeds its padded column stays readable without merging into the status word. |
| Machine output | The existing single update JSON document is unchanged; progress rows appear only in human output. |
| Omitted | No new progress fields or timing details are added; this change only preserves the label/status boundary. |
