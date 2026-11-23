## Core Concepts

A train-merge queue is held by its recorded supervisor only while that session is live. When the train's owner has changed and the prior supervisor has ended, the current train owner's train_merge adopts the queue, settles entries already merged and resumes it. A live prior owner still holds the queue, and only the current train owner may adopt it.
