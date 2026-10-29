-- A quality profile chosen in Melete for a wanted title. Searches and RSS use it over
-- the one the *arr names (quality_profile), and list imports never touch it.
ALTER TABLE wanted_movies ADD COLUMN profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL;
ALTER TABLE wanted_series ADD COLUMN profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL;
