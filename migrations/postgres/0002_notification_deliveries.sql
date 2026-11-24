CREATE TABLE IF NOT EXISTS notification_deliveries (
    chat_id BIGINT NOT NULL,
    match_id BIGINT NOT NULL,
    PRIMARY KEY(chat_id, match_id)
);
