\set ON_ERROR_STOP on
BEGIN;
DO $move$
DECLARE
  image_rule jsonb := '{"transform":"image_resolve_urls","enabled":true,"phase":"response","models":["gpt-image-2.5-sunburst"],"config":{"timeout_seconds":30,"max_bytes":20971520,"roles":["assistant"]}}';
  target_rules jsonb;
  test_rules jsonb;
BEGIN
  SELECT transforms::jsonb INTO STRICT target_rules FROM api_keys
  WHERE id = '71299804-5b24-4bad-975d-2d7f8b794fe4' AND name = 'OpenWebUI' AND enabled = 1 FOR UPDATE;
  SELECT transforms::jsonb INTO STRICT test_rules FROM api_keys
  WHERE id = '714d3ed5-99a4-47a5-8cb0-32ad25fb998e' FOR UPDATE;
  IF NOT test_rules @> jsonb_build_array(image_rule) THEN
    RAISE EXCEPTION 'The previously added Test rule has changed; no update applied';
  END IF;
  IF NOT target_rules @> jsonb_build_array(image_rule) THEN
    UPDATE api_keys SET transforms = (target_rules || jsonb_build_array(image_rule))::text
    WHERE id = '71299804-5b24-4bad-975d-2d7f8b794fe4';
  END IF;
  UPDATE api_keys SET transforms = (
    SELECT COALESCE(jsonb_agg(value ORDER BY ordinal), '[]'::jsonb)::text
    FROM jsonb_array_elements(test_rules) WITH ORDINALITY AS rules(value, ordinal)
    WHERE value <> image_rule
  ) WHERE id = '714d3ed5-99a4-47a5-8cb0-32ad25fb998e';
END $move$;
INSERT INTO state_records (tenant_id, kind, id, value, expires_at)
VALUES ('monoize', 'config_epoch', 'global', '1', NULL)
ON CONFLICT (tenant_id, kind, id)
DO UPDATE SET value = CAST(CAST(state_records.value AS BIGINT) + 1 AS TEXT);
COMMIT;
SELECT id, name, transforms FROM api_keys
WHERE id IN ('71299804-5b24-4bad-975d-2d7f8b794fe4', '714d3ed5-99a4-47a5-8cb0-32ad25fb998e');
