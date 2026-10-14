ALTER TABLE profiles ADD COLUMN icon TEXT;
ALTER TABLE profiles ADD COLUMN color TEXT;

UPDATE profiles
SET color = substr(avatar, 7)
WHERE color IS NULL
  AND avatar LIKE 'color:%'
  AND length(trim(substr(avatar, 7))) > 0;
