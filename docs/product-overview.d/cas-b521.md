## Core Concepts

After a supervisor session ends or its daemon stops, Commander asks the hub for each still-open command card's retained receipt. The hub answers from the project's local store without contacting the daemon, only for the device that sent the command. A stored final outcome is always shown; a queued command is shown as Not sent once the daemon that recorded it is gone, because nothing was typed before its claim. When no receipt remains, the card stays Unconfirmed.
