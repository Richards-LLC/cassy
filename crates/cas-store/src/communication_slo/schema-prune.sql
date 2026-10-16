CREATE TRIGGER IF NOT EXISTS communication_body_observations_prune
AFTER DELETE ON prompt_queue BEGIN
    DELETE FROM communication_body_observations WHERE prompt_id=OLD.id;
END;
