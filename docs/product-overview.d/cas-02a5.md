## Primary Journeys

Cross-machine and in-factory messages report delays honestly. A supervisor's peer message that the peer still has not received two minutes after Cloud accepted it alerts the sender once, with the Cloud message id to check. A message to a worker that is being recycled waits for the worker's successor instead of being abandoned. A lifecycle event queued late says how late it was queued. An idle notice never names a task the worker received after the idle state was detected.
