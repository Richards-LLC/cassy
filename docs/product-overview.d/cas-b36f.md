## Core Concepts

Lane CI attributes failures against the newest target commit already contained in the delivery. A worker's later train merge therefore updates the comparison point instead of leaving inherited train failures blamed on that delivery. An unavailable target can still fall back to the recorded ancestor; genuine delivery-caused failures remain red.
