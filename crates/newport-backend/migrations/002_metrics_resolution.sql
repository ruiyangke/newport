ALTER TABLE samples ADD COLUMN resolution INTEGER NOT NULL DEFAULT 10000;
CREATE INDEX samples_resolution_age ON samples(resolution, at);
