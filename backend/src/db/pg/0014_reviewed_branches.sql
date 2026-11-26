ALTER TABLE subscriptions
  ADD COLUMN review_reviewed_branches TEXT COLLATE "C" NOT NULL DEFAULT '[]';

ALTER TABLE review_runs
  ADD COLUMN scope_note TEXT COLLATE "C";
