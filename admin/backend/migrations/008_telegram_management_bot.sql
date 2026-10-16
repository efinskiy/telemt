CREATE TABLE telegram_bot_settings (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    enabled boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1,
    update_offset bigint,
    username text,
    last_poll_at timestamptz,
    last_error text,
    next_poll_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO telegram_bot_settings(singleton) VALUES(true);
CREATE TABLE telegram_bot_access (
    telegram_id bigint PRIMARY KEY CHECK(telegram_id>0 AND telegram_id<=4503599627370495),
    administrator_id uuid NOT NULL REFERENCES administrators(id) ON DELETE CASCADE
);
CREATE TABLE telegram_bot_sessions (
    telegram_id bigint PRIMARY KEY REFERENCES telegram_bot_access(telegram_id) ON DELETE CASCADE,
    document jsonb NOT NULL,
    expires_at timestamptz NOT NULL DEFAULT now()+interval '15 minutes'
);
CREATE TABLE telegram_bot_actions (
    id uuid PRIMARY KEY,
    telegram_id bigint NOT NULL,
    administrator_id uuid NOT NULL REFERENCES administrators(id) ON DELETE CASCADE,
    revision bigint NOT NULL,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','done','failed','unknown','cancelled')),
    result jsonb NOT NULL DEFAULT '{}',
    artifact_cipher text,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL DEFAULT now()+interval '5 minutes'
);
CREATE INDEX telegram_bot_actions_owner ON telegram_bot_actions(telegram_id,created_at DESC);
CREATE INDEX telegram_bot_actions_retention ON telegram_bot_actions(created_at);
CREATE INDEX telegram_bot_actions_active ON telegram_bot_actions(status,expires_at) WHERE status IN ('pending','running');
CREATE INDEX telegram_bot_actions_artifacts ON telegram_bot_actions(created_at) WHERE artifact_cipher IS NOT NULL;
