### Fixed — Factory

- **Urgent broadcasts keep the 60 s bound for every busy worker** — When an urgent message went to all workers and one was mid tool call, the message was marked delivered as soon as the others got it. The busy worker then saw it only when its call ended: no 60-second interrupt, and no notice. The message now stays open while any worker is held. It is rechecked every 5 s. Workers that already have it are not sent it again, and the busy worker is interrupted after 60 s with "Cassy interrupted your in-flight tool call; this was not the operator."
