-- The version of the Auto DJ rules that planned each transition, as the app sent it
-- back from the plan's `rules`; NULL for apps that don't send it yet.
ALTER TABLE music_transitions ADD COLUMN rules INTEGER CHECK (rules >= 1);
