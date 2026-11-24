CREATE TABLE IF NOT EXISTS notification_deliveries (
    chat_id INTEGER NOT NULL,
    match_id INTEGER NOT NULL,
    PRIMARY KEY(chat_id, match_id)
);
