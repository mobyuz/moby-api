ALTER TABLE users ADD COLUMN display_name TEXT
    CHECK (length(display_name) BETWEEN 1 AND 100);
