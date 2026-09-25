-- v10: how the picked endpoint is chosen — one of the automatic strategies
-- (round_robin | fastest | most_available) or `manual` (the checkmark).
-- Existing rows with a checkmark stay manual; everything else starts
-- automatic (fastest).

ALTER TABLE profiles ADD COLUMN selection_mode TEXT NOT NULL DEFAULT 'fastest';

UPDATE profiles SET selection_mode = 'manual'
  WHERE selected_endpoint_key IS NOT NULL AND selected_endpoint_key != '';
