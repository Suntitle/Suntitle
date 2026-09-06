CREATE TABLE hosts (
    id uuid PRIMARY KEY,
    document jsonb NOT NULL,
    session_id uuid
);
CREATE TABLE nodes (
    id uuid PRIMARY KEY,
    host_id uuid NOT NULL REFERENCES hosts(id),
    revision bigint NOT NULL CHECK (revision > 0),
    document jsonb NOT NULL,
    desired jsonb NOT NULL
);
CREATE TABLE control_commands (
    id uuid PRIMARY KEY,
    host_id uuid NOT NULL REFERENCES hosts(id),
    node_id uuid NOT NULL REFERENCES nodes(id),
    revision bigint NOT NULL CHECK (revision > 0),
    message jsonb NOT NULL,
    completed boolean NOT NULL DEFAULT false,
    UNIQUE (node_id, revision)
);
CREATE INDEX pending_control_commands ON control_commands(host_id, node_id, revision) WHERE NOT completed;
CREATE FUNCTION notify_control_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('control_changes', NEW.host_id::text);
    RETURN NEW;
END;
$$;
CREATE TRIGGER control_change AFTER INSERT OR UPDATE ON control_commands
FOR EACH ROW EXECUTE FUNCTION notify_control_change();
