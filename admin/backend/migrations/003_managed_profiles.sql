CREATE TABLE certificates (
    id uuid PRIMARY KEY, name text NOT NULL, domain text NOT NULL UNIQUE,
    revision bigint NOT NULL DEFAULT 0, material_cipher text,
    info jsonb NOT NULL DEFAULT '{}', issuer_cipher text,
    status text NOT NULL DEFAULT 'empty', last_error text,
    next_attempt timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE certificate_versions (
    certificate_id uuid NOT NULL REFERENCES certificates(id), revision bigint NOT NULL,
    material_cipher text NOT NULL, info jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(certificate_id,revision)
);
CREATE TABLE certificate_orders (
    certificate_id uuid PRIMARY KEY REFERENCES certificates(id), document_cipher text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE profiles (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE, domain text NOT NULL UNIQUE,
    certificate_id uuid NOT NULL REFERENCES certificates(id),
    group_id uuid REFERENCES failover_groups(id), draft_cipher text NOT NULL,
    probe_cipher text NOT NULL,
    draft_revision bigint NOT NULL DEFAULT 1, published_revision bigint NOT NULL DEFAULT 0,
    resume_automatic boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE profile_versions (
    profile_id uuid NOT NULL REFERENCES profiles(id), revision bigint NOT NULL,
    document_cipher text NOT NULL, actor uuid NOT NULL REFERENCES administrators(id),
    created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(profile_id,revision)
);
CREATE TABLE managed_nodes (
    node_id uuid PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    profile_id uuid NOT NULL REFERENCES profiles(id), overrides jsonb NOT NULL DEFAULT '{}',
    enrollment_hash text, enrollment_expires timestamptz, credential_hash text,
    installation_id uuid, revoked boolean NOT NULL DEFAULT false,
    desired_revision bigint NOT NULL DEFAULT 0, applied_revision bigint NOT NULL DEFAULT 0,
    status text NOT NULL DEFAULT 'awaiting_registration', last_seen timestamptz,
    report jsonb NOT NULL DEFAULT '{}', cached_reads_cipher text, cached_at timestamptz,
    UNIQUE(installation_id)
);
CREATE TABLE deployments (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    node_id uuid NOT NULL REFERENCES managed_nodes(node_id) ON DELETE CASCADE,
    profile_revision bigint NOT NULL, certificate_revision bigint NOT NULL,
    bundle_cipher text NOT NULL, digest text NOT NULL, state text NOT NULL DEFAULT 'pending',
    actor uuid REFERENCES administrators(id), result jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX deployment_pending ON deployments(node_id,state);
CREATE TABLE node_commands (
    id uuid PRIMARY KEY, node_id uuid NOT NULL REFERENCES managed_nodes(node_id) ON DELETE CASCADE,
    request_cipher text NOT NULL, response_cipher text,
    state text NOT NULL DEFAULT 'queued', expires_at timestamptz NOT NULL,
    claimed_at timestamptz, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX commands_pending ON node_commands(node_id,state);
CREATE TRIGGER profiles_bound BEFORE INSERT ON profiles FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();
CREATE TRIGGER certificates_bound BEFORE INSERT ON certificates FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();

ALTER TABLE probe_jobs ADD COLUMN web_required boolean NOT NULL DEFAULT false;
