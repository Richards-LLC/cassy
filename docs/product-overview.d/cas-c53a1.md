## Developer workflow

Lane CI records each own/ring invocation and replays it at the delivery head before building the base. This preserves archive identity, selection and partition while avoiding contamination from base dependencies in the shared target directory. Repeated delivery failures that pass at base are caused; failures that pass the identical head invocation remain unattributed and red. Older receipts without replay metadata and empty replays stay unresolved.
