## Core Concepts

A replacement worker on a reassigned task reaches a bound lane without a supervisor override. When the predecessor's branch is untouched, start binds the replacement's own branch automatically. When the predecessor committed work, that branch is kept, the replacement keeps working and delivers, and the supervisor's `rebind_branch` then binds the replacement's branch.
