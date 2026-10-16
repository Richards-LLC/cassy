CREATE TABLE IF NOT EXISTS communication_body_observations (
    prompt_id INTEGER NOT NULL,
    recipient TEXT NOT NULL,
    body_count INTEGER NOT NULL CHECK(body_count > 0),
    PRIMARY KEY(prompt_id, recipient)
);
