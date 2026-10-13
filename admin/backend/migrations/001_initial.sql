CREATE TABLE administrators (
    id uuid PRIMARY KEY, username text NOT NULL UNIQUE,
    password_hash text NOT NULL, totp_cipher text NOT NULL,
    role text NOT NULL CHECK (role IN ('owner','operator','viewer')),
    enabled boolean NOT NULL DEFAULT true, last_totp_step bigint NOT NULL DEFAULT -1,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE sessions (
    token_hash text PRIMARY KEY, administrator_id uuid NOT NULL REFERENCES administrators(id) ON DELETE CASCADE,
    csrf_hash text NOT NULL, csrf_cipher text NOT NULL, expires_at timestamptz NOT NULL, last_seen timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE login_limits (
    key text PRIMARY KEY, failures integer NOT NULL DEFAULT 0, window_start timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE nodes (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE,
    api_url text NOT NULL, token_cipher text NOT NULL, probe_cipher text NOT NULL,
    document jsonb NOT NULL, snapshot jsonb NOT NULL DEFAULT '{}',
    collected_at timestamptz, enabled boolean NOT NULL DEFAULT true
);
CREATE TABLE node_history (
    node_id uuid NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    at timestamptz NOT NULL DEFAULT now(), snapshot jsonb NOT NULL
);
CREATE INDEX node_history_time ON node_history(node_id, at);
CREATE TABLE minute_history (
    node_id uuid NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    minute timestamptz NOT NULL, snapshot jsonb NOT NULL, PRIMARY KEY(node_id, minute)
);
CREATE TABLE operations (
    id uuid PRIMARY KEY, actor uuid NOT NULL REFERENCES administrators(id),
    state text NOT NULL CHECK (state IN ('queued','running','succeeded','partial','failed','unknown')),
    request_cipher text NOT NULL, results jsonb NOT NULL DEFAULT '[]',
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE audit (
    id bigserial PRIMARY KEY, actor uuid, action text NOT NULL, target text NOT NULL,
    outcome jsonb NOT NULL, at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE agents (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE, network text NOT NULL,
    country text NOT NULL CHECK(country='RU'),
    token_hash text NOT NULL, enabled boolean NOT NULL DEFAULT true, last_seen timestamptz
);
CREATE TABLE probe_jobs (
    id uuid PRIMARY KEY, round bigint NOT NULL, agent_id uuid NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    node_id uuid NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, family integer NOT NULL CHECK(family IN (4,6)),
    issued_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL,
    claimed_at timestamptz, completed_at timestamptz, result jsonb,
    UNIQUE(round, agent_id, node_id, family)
);
CREATE INDEX probe_jobs_latest ON probe_jobs(node_id, round DESC);
CREATE TABLE failover_groups (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE, document jsonb NOT NULL,
    token_cipher text NOT NULL, active_node uuid REFERENCES nodes(id),
    automatic boolean NOT NULL DEFAULT false, last_switch timestamptz,
    status text NOT NULL DEFAULT 'disabled', observed jsonb NOT NULL DEFAULT '{}',
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE dns_changes (
    id uuid PRIMARY KEY, group_id uuid NOT NULL REFERENCES failover_groups(id) ON DELETE CASCADE,
    actor uuid, old_node uuid, new_node uuid NOT NULL REFERENCES nodes(id),
    intent jsonb NOT NULL, state text NOT NULL CHECK(state IN ('prepared','writing','verified','failed','unknown')),
    result jsonb NOT NULL DEFAULT '{}', created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE node_health (
    node_id uuid NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    family integer NOT NULL, last_round bigint NOT NULL DEFAULT -1,
    status text NOT NULL DEFAULT 'unknown', PRIMARY KEY(node_id,family)
);

CREATE INDEX audit_expiry ON audit(at);
CREATE INDEX probe_expiry ON probe_jobs(issued_at);
CREATE INDEX operations_pending ON operations(state,created_at);
CREATE FUNCTION enforce_fleet_bound() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE count_rows bigint; existing boolean; maximum integer;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtext(TG_TABLE_NAME));
    EXECUTE format('SELECT EXISTS(SELECT 1 FROM %I WHERE id=$1)',TG_TABLE_NAME) INTO existing USING NEW.id;
    IF existing THEN RETURN NEW; END IF;
    maximum := CASE WHEN TG_TABLE_NAME IN ('nodes','failover_groups') THEN 20 ELSE 100 END;
    EXECUTE format('SELECT count(*) FROM %I',TG_TABLE_NAME) INTO count_rows;
    IF count_rows >= maximum THEN RAISE EXCEPTION 'fleet table capacity reached' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER nodes_bound BEFORE INSERT ON nodes FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();
CREATE TRIGGER groups_bound BEFORE INSERT ON failover_groups FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();
CREATE TRIGGER agents_bound BEFORE INSERT ON agents FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();
CREATE TRIGGER administrators_bound BEFORE INSERT ON administrators FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();

CREATE UNIQUE INDEX enrolled_a_record ON failover_groups((document->>'zone_id'),(document->>'record_a_id')) WHERE coalesce(document->>'record_a_id','')<>'';
CREATE UNIQUE INDEX enrolled_aaaa_record ON failover_groups((document->>'zone_id'),(document->>'record_aaaa_id')) WHERE coalesce(document->>'record_aaaa_id','')<>'';
CREATE TABLE dns_record_cooldown (
    zone_id text NOT NULL, record_id text NOT NULL, last_switch timestamptz NOT NULL,
    PRIMARY KEY(zone_id,record_id)
);
