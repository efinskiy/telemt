CREATE TABLE notification_settings (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    revision bigint NOT NULL DEFAULT 1,
    enabled boolean NOT NULL DEFAULT false,
    chat_id text NOT NULL DEFAULT '',
    token_cipher text,
    ip_rotation boolean NOT NULL DEFAULT true,
    server_switch boolean NOT NULL DEFAULT true
);
INSERT INTO notification_settings(singleton) VALUES(true);

-- Event snapshots survive removal of their original group or node.
CREATE TABLE notification_deliveries (
    id uuid PRIMARY KEY,
    dns_change uuid UNIQUE,
    revision bigint NOT NULL,
    kind text NOT NULL CHECK(kind IN ('ip_rotation','server_switch','test')),
    message text NOT NULL,
    status text NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','sent','failed','cancelled')),
    attempts integer NOT NULL DEFAULT 0,
    error text,
    next_attempt timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    sent_at timestamptz
);
CREATE INDEX notification_pending ON notification_deliveries(next_attempt) WHERE status='queued';
