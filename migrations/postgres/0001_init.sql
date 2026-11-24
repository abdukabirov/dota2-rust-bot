CREATE TABLE IF NOT EXISTS subscriptions (
    chat_id BIGINT NOT NULL,
    match_id BIGINT NOT NULL,
    PRIMARY KEY(chat_id, match_id)
);
CREATE TABLE IF NOT EXISTS update_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS widget_messages (
    message_id BIGINT PRIMARY KEY,
    match_id BIGINT NOT NULL
);
