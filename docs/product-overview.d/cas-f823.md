## Core Concepts

- **Train main sync** - An open release train that lacks commits on the integration branch takes them in a merge on each CI poll, published to origin, and the workers with unfinished deliveries on the lane are told. A frozen train is left alone. A conflicting merge changes nothing and files one sync task under the train naming the files, plus one supervisor notice.
