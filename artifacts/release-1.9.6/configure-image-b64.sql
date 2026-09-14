\set ON_ERROR_STOP on
BEGIN;
DO $configure$
DECLARE changed_count integer;
BEGIN
  UPDATE api_keys
  SET transforms = '[{"transform":"image_resolve_urls","enabled":true,"phase":"response","models":["gpt-image-2.5-sunburst"],"config":{"timeout_seconds":30,"max_bytes":20971520,"roles":["assistant"]}}]'
  WHERE id = '714d3ed5-99a4-47a5-8cb0-32ad25fb998e' AND transforms::jsonb = '[]'::jsonb;
  GET DIAGNOSTICS changed_count = ROW_COUNT;
  IF changed_count <> 1 THEN RAISE EXCEPTION 'Expected one API key with unchanged empty transforms; updated %', changed_count; END IF;
END $configure$;
INSERT INTO state_records (tenant_id, kind, id, value, expires_at)
VALUES ('monoize', 'config_epoch', 'global', '1', NULL)
ON CONFLICT (tenant_id, kind, id)
DO UPDATE SET value = CAST(CAST(state_records.value AS BIGINT) + 1 AS TEXT);
COMMIT;
SELECT id, name, transforms FROM api_keys WHERE id = '714d3ed5-99a4-47a5-8cb0-32ad25fb998e';
