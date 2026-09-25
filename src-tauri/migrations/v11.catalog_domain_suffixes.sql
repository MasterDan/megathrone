-- v11: the default catalog gained whole-domain suffix rules (the CDN
-- domains its URL rules enumerate subdomains of). They reach existing
-- databases matched by category name, skipped when already present;
-- renamed or deleted categories simply match nothing.

INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
    SELECT c.id, 'domain_suffix', 'googlevideo.com', 1 FROM test_site_categories c
    WHERE c.name = 'Google Video'
      AND NOT EXISTS (SELECT 1 FROM test_sites n
                      WHERE n.category_id = c.id
                        AND n.rule_type = 'domain_suffix' AND n.value = 'googlevideo.com');
INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
    SELECT c.id, 'domain_suffix', 'ytimg.com', 1 FROM test_site_categories c
    WHERE c.name = 'YouTube'
      AND NOT EXISTS (SELECT 1 FROM test_sites n
                      WHERE n.category_id = c.id
                        AND n.rule_type = 'domain_suffix' AND n.value = 'ytimg.com');
INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
    SELECT c.id, 'domain_suffix', 'ggpht.com', 1 FROM test_site_categories c
    WHERE c.name = 'YouTube'
      AND NOT EXISTS (SELECT 1 FROM test_sites n
                      WHERE n.category_id = c.id
                        AND n.rule_type = 'domain_suffix' AND n.value = 'ggpht.com');
INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
    SELECT c.id, 'domain_suffix', 't.me', 1 FROM test_site_categories c
    WHERE c.name = 'Telegram'
      AND NOT EXISTS (SELECT 1 FROM test_sites n
                      WHERE n.category_id = c.id
                        AND n.rule_type = 'domain_suffix' AND n.value = 't.me');
INSERT INTO test_sites (category_id, rule_type, value, test_enabled)
    SELECT c.id, 'domain_suffix', 'twimg.com', 1 FROM test_site_categories c
    WHERE c.name = 'Social'
      AND NOT EXISTS (SELECT 1 FROM test_sites n
                      WHERE n.category_id = c.id
                        AND n.rule_type = 'domain_suffix' AND n.value = 'twimg.com');
